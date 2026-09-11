use std::fs;
use std::process::Command;
use interoptopus::Interop;
use game_wasm::my_inventory;

fn main() {
    println!("📦 Creating SDK output folders...");
    fs::create_dir_all("sdks/csharp").unwrap();
    fs::create_dir_all("sdks/python").unwrap();
    fs::create_dir_all("sdks/c_cpp").unwrap();
    fs::create_dir_all("sdks/swift").unwrap();
    fs::create_dir_all("sdks/kotlin").unwrap();
    fs::create_dir_all("sdks/ts").unwrap();

    let inventory = my_inventory();

    // 1. Interoptopus Generators
    use interoptopus_backend_csharp::{Generator as CSharpGen, Config as CSharpConfig};
    CSharpGen::new(CSharpConfig::default(), inventory.clone())
        .write_file("sdks/csharp/GameServerSDK.cs").unwrap();

    use interoptopus_backend_cpython::{Generator as PythonGen, Config as PythonConfig};
    PythonGen::new(PythonConfig::default(), inventory.clone())
        .write_file("sdks/python/game_server_sdk.py").unwrap();

    use interoptopus_backend_c::{Generator as CGen, Config as CConfig};
    CGen::new(CConfig::default(), inventory.clone())
        .write_file("sdks/c_cpp/game_server_sdk.h").unwrap();

    // 2. Typeshare CLI Generators
    let run_typeshare = |lang: &str, output: &str| {
        let status = Command::new("typeshare")
            .args(["./src", "--lang", lang, "--output-file", output])
            .status()
            .expect("Failed to execute typeshare CLI. Ensure `cargo install typeshare-cli` was run.");
        assert!(status.success());
    };

    run_typeshare("swift", "sdks/swift/GameSDK.swift");
    run_typeshare("kotlin", "sdks/kotlin/GameSDK.kt");
    run_typeshare("typescript", "sdks/ts/types.ts");

    println!("✅ All SDKs generated successfully in ./sdks/!");
}
