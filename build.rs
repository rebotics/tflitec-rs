extern crate bindgen;

use std::fs;
use std::env;
use std::fmt::Debug;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;
use flate2::read::GzDecoder;

use const_format::formatcp;

const TF_VER: &str = "2.14.0";

const TAG: &str = formatcp!("v{TF_VER}");
const TF_GIT_URL: &str = "https://github.com/tensorflow/tensorflow.git";

const ANDROID_BIN_DOWNLOAD_URL: &str = formatcp!(
    "https://repo1.maven.org/maven2/org/tensorflow/tensorflow-lite/{TF_VER}/tensorflow-lite-{TF_VER}.aar"
);
const ANDROID_BIN_FLEX_DOWNLOAD_URL: &str = formatcp!(
    "https://repo1.maven.org/maven2/org/tensorflow/tensorflow-lite-select-tf-ops/{TF_VER}/tensorflow-lite-select-tf-ops-{TF_VER}.aar"
);

// Download URL for the iOS cannot be constucted dynamically and should be replaced manually
// with a new release of TFLite
const IOS_BIN_DOWNLOAD_URL: &str = formatcp!(
    "https://dl.google.com/tflite-nightly/ios/prod/tensorflow/lite/release/ios/nightly/807/20230224-035015/TensorFlowLiteC/0.0.1-nightly.20230224/TensorFlowLiteC-0.0.1-nightly.20230224.tar.gz"
);

fn target_os() -> String {
    env::var("CARGO_CFG_TARGET_OS").expect("Unable to get TARGET_OS")
}

fn dll_extension() -> &'static str {
    match target_os().as_str() {
        "macos" => "dylib",
        "windows" => "dll",
        _ => "so",
    }
}

fn dll_prefix() -> &'static str {
    match target_os().as_str() {
        "windows" => "",
        _ => "lib",
    }
}

fn copy_or_overwrite<P: AsRef<Path> + Debug, Q: AsRef<Path> + Debug>(src: P, dest: Q) {
    let src_path: &Path = src.as_ref();
    let dest_path: &Path = dest.as_ref();

    if dest_path.exists() {
        if dest_path.is_file() {
            std::fs::remove_file(dest_path).expect("Cannot remove file");
        } else {
            std::fs::remove_dir_all(dest_path).expect("Cannot remove directory");
        }
    }

    if src_path.is_dir() {
        let options = fs_extra::dir::CopyOptions {
            copy_inside: true,
            ..fs_extra::dir::CopyOptions::new()
        };
        fs_extra::dir::copy(src_path, dest_path, &options).unwrap_or_else(|e| {
            panic!(
                "Cannot copy directory from {:?} to {:?}. Error: {}",
                src, dest, e
            )
        });
    } else {
        std::fs::copy(src_path, dest_path).unwrap_or_else(|e| {
            panic!(
                "Cannot copy file from {:?} to {:?}. Error: {}",
                src, dest, e
            )
        });
    }
}

fn prepare_tensorflow_source(tf_src_path: &Path) {
    let complete_clone_hint_file = tf_src_path.join(".complete_clone");
    if !complete_clone_hint_file.exists() {
        if tf_src_path.exists() {
            std::fs::remove_dir_all(tf_src_path).expect("Cannot clean tf_src_path");
        }
        let mut git = std::process::Command::new("git");
        git.arg("clone")
            .args(["--depth", "1"])
            .arg("--shallow-submodules")
            .args(["--branch", TAG])
            .arg("--single-branch")
            .arg(TF_GIT_URL)
            .arg(tf_src_path.to_str().unwrap());
        println!("Git clone started");
        let start = Instant::now();
        if !git.status().expect("Cannot execute `git clone`").success() {
            panic!("git clone failed");
        }
        std::fs::File::create(complete_clone_hint_file).expect("Cannot create clone hint file!");
        println!("Clone took {:?}", Instant::now() - start);
    }
}

fn get_lib_name() -> String {
    let ext = dll_extension();
    let lib_prefix = dll_prefix();

    match target_os().as_str() {
        "android" => {
            format!("{}tensorflowlite_jni.{}", lib_prefix, ext)
        },
        "ios" => {
            String::from("TensorFlowLiteC.framework")
        },
        _ => {
            format!("{}tensorflowlite_c.{}", lib_prefix, ext)
        }
    }
}

fn get_flex_name() -> String {
    let ext = dll_extension();
    let lib_prefix = dll_prefix();

    match target_os().as_str() {
        "android" => {
            format!("{}tensorflowlite_flex_jni.{}", lib_prefix, ext)
        },
        "ios" => {
            String::from("TensorFlowLiteSelectTfOps.framework")
        }
        _ => {
            format!("{}tensorflowlite_flex.{}", lib_prefix, ext)
        }
    }
}

fn lib_output_path() -> PathBuf {
    out_dir().join(get_lib_name())
}

fn flex_output_path() -> PathBuf {
    out_dir().join(get_flex_name())
}

fn out_dir() -> PathBuf {
    PathBuf::from(env::var("OUT_DIR").unwrap())
}

fn prepare_for_docsrs() {
    // Docs.rs cannot access to network, use resource files
    let library_path = out_dir().join("libtensorflowlite_c.so");
    let bindings_path = out_dir().join("bindings.rs");

    let mut unzip = std::process::Command::new("unzip");
    let root = std::path::PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    unzip
        .arg(root.join("build-res/docsrs_res.zip"))
        .arg("-d")
        .arg(out_dir());
    if !(unzip
        .status()
        .unwrap_or_else(|_| panic!("Cannot execute unzip"))
        .success()
        && library_path.exists()
        && bindings_path.exists())
    {
        panic!("Cannot extract docs.rs resources")
    }
}

fn generate_binding_ios() {
    let mut builder = bindgen::Builder::default();

    let headers_path = out_dir().join("TensorFlowLiteC.framework/Headers");
    let header_path = headers_path.join("c_api.h");

    builder = builder.header(
        header_path.to_str().unwrap()
    );

    if cfg!(feature = "xnnpack") {
        let header_path = headers_path.join("xnnpack_delegate.h");

        builder = builder.header(
            header_path.to_str().unwrap()
        );
    }

    let bindings = builder
        .clang_arg(format!("-I{}", headers_path.to_str().unwrap()))
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    bindings
        .write_to_file(out_dir().join("bindings.rs"))
        .expect("Couldn't write bindings!");
}

fn generate_bindings(tf_src_path: PathBuf) {
    let mut builder = bindgen::Builder::default().header(
        tf_src_path
            .join("tensorflow/lite/c/c_api.h")
            .to_str()
            .unwrap(),
    );
    if cfg!(feature = "xnnpack") {
        builder = builder.header(
            tf_src_path
                .join("tensorflow/lite/delegates/xnnpack/xnnpack_delegate.h")
                .to_str()
                .unwrap(),
        );
    }

    let bindings = builder
        .clang_arg(format!("-I{}", tf_src_path.to_str().unwrap()))
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    bindings
        .write_to_file(out_dir().join("bindings.rs"))
        .expect("Couldn't write bindings!");
}

fn download_ios(
    url: &str,
    save_path: &Path,
    filename: &str,
) {
    std::fs::create_dir_all(&save_path).unwrap();

    let framework_name = filename.split(".").nth(0).unwrap();
    let archive_path = save_path.join(format!("{framework_name}.tar.gz"));

    println!("Starting to download archive with {}...", filename);
    let start = Instant::now();
    download_file(url, &archive_path);
    println!(
        "Finished downloading archive with {}, took: {:?}",
        filename,
        Instant::now() - start,
    );

    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Unable to get TARGET_ARCH");
    let arch = match arch.as_str() {
        "aarch64" => "ios-arm64".to_string(),
        _ => panic!("'{}' not supported", arch),
    };

    let file = fs::File::open(archive_path).unwrap();
    let decompressed = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decompressed);

    let arhive_filepath = format!(
        "Frameworks/{framework_name}.xcframework/{arch}/{filename}/"
    );

    archive.entries().unwrap().for_each(|entry| {
        let mut file = entry.unwrap();

        let file_path = file.path().unwrap();
        let file_path = file_path.to_str().unwrap();

        if file_path.contains(&arhive_filepath) {
            let file_path = file_path.split("/").skip(4).collect::<Vec<_>>().join("/");

            file.unpack(save_path.join(file_path)).unwrap();
        }
    });
}

fn download_android(
    url: &str,
    save_path: &Path,
    filename: &str,
) {
    std::fs::create_dir_all(&save_path).unwrap();
    let aar_path = save_path.join("android_lib");

    println!("Starting to download archive with {}...", filename);
    let start = Instant::now();
    download_file(url, &aar_path);
    println!(
        "Finished downloading archive with {}, took: {:?}",
        filename,
        Instant::now() - start,
    );

    let file = fs::File::open(aar_path).unwrap();

    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Unable to get TARGET_ARCH");
    let arch = match arch.as_str() {
        "aarch64" => "arm64-v8a".to_string(),
        "armv7" => "armeabi-v7a".to_string(), 
        "x86" => arch,
        "x86_64" => arch,
        _ => panic!("'{}' not supported", arch),
    };

    let mut archive = zip::ZipArchive::new(file).unwrap();
    let archive_file_path = format!("jni/{}/{}", arch, filename);

    let mut archive_file = archive.by_name(&archive_file_path).expect(
        "No file found in the AAR"
    );
    let mut buff: Vec<u8> = vec![];
    archive_file.read_to_end(&mut buff).unwrap();

    let file_path = save_path.join(filename);
    let mut file = fs::File::create(&file_path).unwrap();
    file.write_all(&buff).unwrap();
}

fn download_and_install(tf_src_path: &Path) {
    // Copy prebuilt libraries to given path
    {
        let libname = get_lib_name();
        let flexname = get_flex_name();

        let save_path = tf_src_path.join("pkgs");

        match target_os().as_str() {
            "android" => {
                download_android(ANDROID_BIN_DOWNLOAD_URL, &save_path, &libname);
                #[cfg(feature = "flex_delegate")]
                download_android(ANDROID_BIN_FLEX_DOWNLOAD_URL, &save_path, &flexname);
            },
            "ios" => {
                download_ios(IOS_BIN_DOWNLOAD_URL, &save_path, &libname);
            },
            _ => {
                panic!("Only iOS and Android are supported for now");
            }
        };

        let lib_src_path = PathBuf::from(&save_path).join(&libname);
        let lib_output_path = lib_output_path();

        copy_or_overwrite(&lib_src_path, &lib_output_path);

        #[cfg(all(android, feature = "flex_delegate"))] {
            let flex_src_path = PathBuf::from(&save_path).join(&flexname);
            let flex_output_path = flex_output_path();

            copy_or_overwrite(&flex_src_path, &flex_output_path);
        }
    }
}

fn download_file(url: &str, path: &Path) {
    let mut easy = curl::easy::Easy::new();
    let output_file = std::fs::File::create(path).unwrap();
    let mut writer = std::io::BufWriter::new(output_file);
    easy.url(url).unwrap();
    easy.write_function(move |data| Ok(writer.write(data).unwrap()))
        .unwrap();
    easy.perform().unwrap_or_else(|e| {
        std::fs::remove_file(path).unwrap(); // Delete corrupted or empty file
        panic!("Error occurred while downloading from {}: {:?}", url, e);
    });
}

fn main() {
    let out_path = out_dir();
    let os = target_os();

    match os.as_str() {
        "android" => {
            println!("cargo:rustc-link-search=native={}", out_path.display());
            println!("cargo:rustc-link-lib=dylib=tensorflowlite_jni");
        }
        "ios" => {
            println!("cargo:rustc-link-search=framework={}", out_path.display());
            println!("cargo:rustc-link-lib=framework=TensorFlowLiteC");
            println!("cargo:rustc-link-lib=c++");
        }
        _ => {
            panic!("Only iOS and Android are supported for now");
            // println!("cargo:rustc-link-search=native={}", out_path.display());
            // println!("cargo:rustc-link-lib=dylib=tensorflowlite_c");

            // #[cfg(feature = "flex_delegate")]
            // println!("cargo:rustc-link-lib=dylib=tensorflowlite_flex");
        }
    }

    if env::var("DOCS_RS") == Ok(String::from("1")) {
        // docs.rs cannot access to network, use resource files
        prepare_for_docsrs();
    } else {
        let tf_src_path = out_path.join(format!("tensorflow_{}", TAG));

        if os != "ios" {
            prepare_tensorflow_source(tf_src_path.as_path());
            download_and_install(&tf_src_path);

            generate_bindings(tf_src_path);
        } else {
            download_and_install(&tf_src_path);

            generate_binding_ios();
        }
    }
}
