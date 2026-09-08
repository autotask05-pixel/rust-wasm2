use std::fs;
use std::process::Command;
use interoptopus::Interop;
use game_wasm::my_inventory;

fn main() {
    println!("📦 Creating SDK output folders...");
    fs::create_dir_all("sdks/csharp").unwrap();
    fs::create_dir_all("sdks/python").unwrap();
    fs::create_dir_all("sdks/c_cpp").unwrap();
    fs::create_dir_all("sdks/swift/Sources/GameSDK").unwrap();
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
            .expect("Failed to execute typeshare CLI.");
        assert!(status.success());
    };

    run_typeshare("swift", "sdks/swift/Sources/GameSDK/GameSDK.swift");
    run_typeshare("kotlin", "sdks/kotlin/GameSDK.kt");
    run_typeshare("typescript", "sdks/ts/types.ts");

    // 3. Auto-Generate Package Manifests
    println!("📄 Emitting Package Manifests (npm, NuGet, Swift, Unity)...");

    // A. TypeScript / npm package.json
    fs::write("sdks/ts/package.json", r#"{
  "name": "@game-engine/sdk",
  "version": "0.1.0",
  "description": "Zero-copy TypeScript client for Edge Multiplayer Engine",
  "main": "types.ts",
  "types": "types.ts",
  "license": "MIT"
}"#).unwrap();

    // B. C# / NuGet / Unity UPM package.json + .csproj
    fs::write("sdks/csharp/package.json", r#"{
  "name": "com.gameengine.sdk",
  "version": "0.1.0",
  "displayName": "Game Engine Client SDK",
  "description": "Zero-copy C# bindings for Edge Game Engine",
  "unity": "2021.3"
}"#).unwrap();

    fs::write("sdks/csharp/GameServerSDK.csproj", r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>netstandard2.1</TargetFramework>
    <PackageId>GameEngine.SDK</PackageId>
    <Version>0.1.0</Version>
    <Authors>Engine Team</Authors>
  </PropertyGroup>
</Project>"#).unwrap();

    // C. Swift Package Manager (Package.swift)
    fs::write("sdks/swift/Package.swift", r#"// swift-tools-version: 5.7
import PackageDescription

let package = Package(
    name: "GameSDK",
    products: [
        .library(name: "GameSDK", targets: ["GameSDK"]),
    ],
    targets: [
        .target(name: "GameSDK", path: "Sources/GameSDK"),
    ]
)"#).unwrap();

    // D. Python pyproject.toml
    fs::write("sdks/python/pyproject.toml", r#"[build-system]
requires = ["setuptools>=61.0"]
build-backend = "setuptools.build_meta"

[project]
name = "game_server_sdk"
version = "0.1.0"
description = "Zero-copy Python bindings for Edge Game Engine"
readme = "README.md"
authors = [{ name = "Engine Team" }]
dependencies = []
"#).unwrap();
    fs::write("sdks/python/README.md", "# Game Server Python SDK\nZero-copy ctypes bindings.").unwrap();

    println!("✅ All SDKs & Package Manifests emitted successfully in ./sdks/!");
}
