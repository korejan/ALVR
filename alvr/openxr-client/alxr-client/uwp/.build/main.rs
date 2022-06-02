#![forbid(unsafe_code)]

use std::{
    convert::{TryFrom, TryInto},
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use toml::{value::Map, Value};

mod shared;
use shared::*;

fn main() -> Result<(), Box<dyn Error>> {
    let cargo_config = get_cargo_config()?;

    // Construct the templates directory
    let mut templates_dir = cargo_config.package_root.clone();
    templates_dir.push(TEMPLATES_DIR);
    let templates_dir = templates_dir;

    // Rerun this build script when any of the inputs change (template files, Cargo.toml)
    let mut appx_manifest_file = templates_dir.clone();
    appx_manifest_file.push("AppxManifest.xml");
    let appx_manifest_file = appx_manifest_file;
    println!("cargo:rerun-if-changed={}", appx_manifest_file.display());

    let mut mapping_file = templates_dir;
    mapping_file.push("FileMapping.ini");
    let mapping_file = mapping_file;
    println!("cargo:rerun-if-changed={}", mapping_file.display());

    let mut cargo_toml_file = cargo_config.package_root.clone();
    cargo_toml_file.push("Cargo.toml");
    let cargo_toml_file = cargo_toml_file;
    println!("cargo:rerun-if-changed={}", cargo_toml_file.display());

    let cargo_pkg_config = get_cargo_pkg_config()?;
    let appx_config = get_appx_config(&cargo_config.package_root, &cargo_pkg_config)?;

    // Generate AppxManifest.xml file from template
    let appx_manifest = fs::read_to_string(&appx_manifest_file)?;
    let appx_manifest = generate_appx_manifest(appx_manifest, &appx_config, &cargo_pkg_config)?;
    let mut appx_manifest_out = cargo_config.target_dir.clone();
    appx_manifest_out.push("AppxManifest.xml");
    fs::write(&appx_manifest_out, &appx_manifest)?;

    // Generate FileMapping.ini file from template
    let file_mapping = fs::read_to_string(&mapping_file)?;
    let file_mapping = generate_mapping_file(file_mapping, &cargo_config, &cargo_pkg_config)?;
    let mut file_mapping_out = cargo_config.target_dir;
    file_mapping_out.push("FileMapping.ini");
    fs::write(&file_mapping_out, &file_mapping)?;

    Ok(())
}

struct CargoConfig {
    package_root: PathBuf,
    target_dir: PathBuf,
}

fn get_cargo_config() -> Result<CargoConfig, Box<dyn Error>> {
    // Find the cargo package root
    let package_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);

    // Find location where the executable artifacts are compiled to. This should be
    // at <cargo package root>\target\<target triple>\<debug|release>
    let mut target_dir = PathBuf::from(env::var("OUT_DIR")?);
    target_dir.pop();
    target_dir.pop();
    target_dir.pop();
    let target_dir = target_dir;

    // Issue a warning if the target directory doesn't match the expected pattern
    if !target_dir.ends_with("debug") && !target_dir.ends_with("release") {
        println!(
            "cargo:warning=Unexpected target directory {}",
            target_dir.display()
        );
    }

    Ok(CargoConfig {
        package_root,
        target_dir,
    })
}

enum Arch {
    X86,   // "i686-uwp-windows-msvc"
    X64,   // "x86_64-uwp-windows-msvc"
    Arm,   // "thumbv7a-uwp-windows-msvc"
    Arm64, // "aarch64-uwp-windows-msvc"
}

impl Arch {
    fn display(&self) -> &'static str {
        match *self {
            Arch::X86 => "x86",
            Arch::X64 => "x64",
            Arch::Arm => "arm",
            Arch::Arm64 => "arm64",
        }
    }
}

impl TryFrom<String> for Arch {
    type Error = Box<dyn Error>;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "i686-uwp-windows-msvc" => Ok(Arch::X86),
            "x86_64-uwp-windows-msvc" => Ok(Arch::X64),
            "thumbv7a-uwp-windows-msvc" => Ok(Arch::Arm),
            "aarch64-uwp-windows-msvc" => Ok(Arch::Arm64),
            _ => Err(format!("Unknown target {}", value).into()),
        }
    }
}

struct CargoPkgConfig {
    version: String,
    name: String,
    executable: String,
    arch: Arch,
}

fn get_cargo_pkg_config() -> Result<CargoPkgConfig, Box<dyn Error>> {
    let version = env::var("CARGO_PKG_VERSION")?;
    let name = env::var("CARGO_PKG_NAME")?;
    let mut executable = name.clone();
    executable.push_str(".exe");
    let arch: Arch = env::var("TARGET")?.try_into()?;

    Ok(CargoPkgConfig {
        version,
        name,
        executable,
        arch,
    })
}

#[derive(Debug)]
struct AppxConfig {
    identity_name: String,
    identity_publisher: String,
    identity_version: String,
    phone_product_id: Option<String>,
    display_name: String,
    publisher_display_name: String,
    app_display_name: String,
    app_description: String,
}

fn get_appx_config(
    cargo_package_root: &Path,
    cargo_pkg_config: &CargoPkgConfig,
) -> Result<AppxConfig, Box<dyn Error>> {
    let mut manifest_file = PathBuf::from(cargo_package_root);
    manifest_file.push("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_file)?;

    let root = manifest.parse::<Value>()?;
    let root = root.as_table().ok_or("Invalid Cargo.toml file")?;
    // Navigate to 'package'/'metadata'/'appxmanifest' table
    let package = root
        .get("package")
        .and_then(|val| val.as_table())
        .ok_or("Missing [package] table")?;
    let metadata = package
        .get("metadata")
        .and_then(|val| val.as_table())
        .ok_or("Missing [metadata] table")?;
    let appxmanifest = metadata
        .get("appxmanifest")
        .and_then(|val| val.as_table())
        .ok_or("Missing [appxmanifest] table")?;

    // Extract identity name
    let identity_name = get_value(appxmanifest, PACKAGE_IDENTITY_NAME_KEY)?;
    warn_if_default(
        PACKAGE_IDENTITY_NAME_KEY,
        &identity_name,
        PACKAGE_IDENTITY_NAME_DEFAULT,
    );

    // Extract identity publisher
    let identity_publisher = get_value(appxmanifest, PACKAGE_IDENTITY_PUBLISHER_KEY)?;
    warn_if_default(
        PACKAGE_IDENTITY_PUBLISHER_KEY,
        &identity_publisher,
        PACKAGE_IDENTITY_PUBLISHER_DEFAULT,
    );

    // Extract identity version; fall back to package version when missing
    let identity_version = if let Some(version) = appxmanifest.get(PACKAGE_IDENTITY_VERSION_KEY) {
        let version = version
            .as_str()
            .ok_or(format!("Invalid '{}' key", PACKAGE_IDENTITY_VERSION_KEY))?
            .to_owned();
        warn_if_default(
            PACKAGE_IDENTITY_VERSION_KEY,
            &version,
            PACKAGE_IDENTITY_VERSION_DEFAULT,
        );
        version
    } else {
        let mut version = cargo_pkg_config.version.clone();
        version.push_str(".0");
        version
    };

    // Extract phone product id; when missing store `None` to remove Windows 10 Mobile
    // support
    let phone_product_id = if let Some(id) = appxmanifest.get(PACKAGE_PHONE_ID_KEY) {
        let id = id
            .as_str()
            .ok_or(format!("Invalid '{}' key", PACKAGE_PHONE_ID_KEY))?
            .to_owned();
        warn_if_default(PACKAGE_PHONE_ID_KEY, &id, PACKAGE_PHONE_ID_DEFAULT);
        Some(id)
    } else {
        None
    };

    // Extract display name; fall back to package name when missing
    let display_name = if let Some(name) = appxmanifest.get(PACKAGE_DISPLAY_NAME_KEY) {
        let name = name
            .as_str()
            .ok_or(format!("Invalid '{}' key", PACKAGE_DISPLAY_NAME_KEY))?
            .to_owned();
        warn_if_default(
            PACKAGE_DISPLAY_NAME_KEY,
            &name,
            PACKAGE_DISPLAY_NAME_DEFAULT,
        );
        name
    } else {
        cargo_pkg_config.name.clone()
    };

    // Extract publisher display name
    let publisher_display_name = get_value(appxmanifest, PACKAGE_PUBLISHER_DISPLAY_NAME_KEY)?;
    warn_if_default(
        PACKAGE_PUBLISHER_DISPLAY_NAME_KEY,
        &publisher_display_name,
        PACKAGE_PUBLISHER_DISPLAY_NAME_DEFAULT,
    );

    // Extract application display name
    let app_display_name = get_value(appxmanifest, PACKAGE_VISUAL_DISPLAY_NAME_KEY)?;
    warn_if_default(
        PACKAGE_VISUAL_DISPLAY_NAME_KEY,
        &app_display_name,
        PACKAGE_VISUAL_DISPLAY_NAME_DEFAULT,
    );

    // Extract application description
    let app_description = get_value(appxmanifest, PACKAGE_VISUAL_DESCRIPTION_KEY)?;
    warn_if_default(
        PACKAGE_VISUAL_DESCRIPTION_KEY,
        &app_description,
        PACKAGE_VISUAL_DESCRIPTION_DEFAULT,
    );

    Ok(AppxConfig {
        identity_name,
        identity_publisher,
        identity_version,
        phone_product_id,
        display_name,
        publisher_display_name,
        app_display_name,
        app_description,
    })
}

fn get_value(manifest: &Map<String, Value>, key: &str) -> Result<String, Box<dyn Error>> {
    Ok(manifest
        .get(key)
        .and_then(|val| val.as_str())
        .ok_or(format!("Missing or invalid '{}' key", key))?
        .to_owned())
}

fn warn_if_default(key: &str, value: &str, default: &str) {
    if value == default {
        println!("cargo:warning=Metadata key '{}' uses default value", key);
    }
}

fn generate_appx_manifest(
    template: String,
    appx_config: &AppxConfig,
    cargo_pkg_config: &CargoPkgConfig,
) -> Result<String, Box<dyn Error>> {
    // Insert disclaimer
    let mut manifest = template.replace("$generated-content-disclaimer$", XML_DISCLAIMER);

    // Fill in Windows 10 Mobile product ID or remove Windows 10 Mobile support when
    // missing
    if let Some(mob_ver) = &appx_config.phone_product_id {
        manifest = manifest
            .replace("$win10mob-begin$", "")
            .replace("$win10mob-end$", "")
            .replace("$appx-identity-phoneproductid$", mob_ver);
    } else {
        while let Some(begin) = manifest.find("$win10mob-begin$") {
            if let Some(end) = manifest
                .find("$win10mob-end$")
                .map(|pos| pos + "$win10mob-end$".len())
            {
                if end < begin {
                    return Err("Unmatched '$win10mobile_end$' placeholder".into());
                }
                manifest.replace_range(begin..end, "");
            } else {
                return Err("Unmatched '$win10mobile_begin$' placeholder".into());
            }
        }
    }
    // Sanity check
    if manifest
        .find("$win10mob-begin$")
        .or_else(|| manifest.find("$win10mob-end$"))
        .or_else(|| manifest.find("$appx-identity-phoneproductid$"))
        .is_some()
    {
        return Err("Failed to substitute all Windows 10 Mobile placeholders".into());
    }

    // Replace Package Identity Name
    manifest = manifest.replace("$appx-identity-name$", &appx_config.identity_name);

    // Replace Package Identity Architecture
    manifest = manifest.replace("$appx-identity-arch$", cargo_pkg_config.arch.display());

    // Replace Package Identity Publisher
    manifest = manifest.replace("$appx-identity-publisher$", &appx_config.identity_publisher);

    // Replace Package Identity Version
    manifest = manifest.replace("$appx-identity-version$", &appx_config.identity_version);

    // Replace Package Property Display Name
    manifest = manifest.replace("$appx-prop-displayname$", &appx_config.display_name);

    // Replace Package Property Publisher Display Name
    manifest = manifest.replace(
        "$appx-prop-publisherdisplayname$",
        &appx_config.publisher_display_name,
    );

    // Replace Package Application Executable
    manifest = manifest.replace(
        "$appx-application-executable$",
        &cargo_pkg_config.executable,
    );

    // Replace application display name
    manifest = manifest.replace(
        "$appx-application-displayname$",
        &appx_config.app_display_name,
    );

    // Replace application description
    manifest = manifest.replace(
        "$appx-application-description$",
        &appx_config.app_description,
    );

    Ok(manifest)
}

// Mapping files do not support comments, so the comments go here:
//
// This file serves as a template used by the build system to generate a mapping file
// passed on to *MakeAppx.exe* to control which artifacts go into the final .appx package,
// and how they are named.
//
// Each line describes a pair of file paths in quotation marks, separated by either spaces
// or tabs, where the left-hand side names the local (on disk) location, and the right-hand
// side designates the .appx-relative path name.
//
// The build system uses this template to generate the final *FileMapping.ini* file when
// building a target. Clients can adjust this file to control .appx package contents.
//
// With several artifacts being generated by the build system into configuration-specific
// locations, the following placeholders are supported to allow writing a generic template:
//
// Left-hand side:
// * $target_dir$: The local path where artifacts are generated into.
// * $cargo_package_root$: The root directory of the cargo package.
// * $executable$: File name of the executable image.
//
// Righ-hand side:
// * $file_name$: The file name (without a path) derived from the left-hand side after
//                expansion.
//
fn generate_mapping_file(
    template: String,
    cargo_config: &CargoConfig,
    cargo_pkg_config: &CargoPkgConfig,
) -> Result<String, Box<dyn Error>> {
    let mut output = Vec::<String>::new();
    for line in template.lines() {
        if let Some((lhs, rhs)) = parse_mapping(line) {
            // Expand left-hand side
            let lhs = lhs.replace(
                "$target_dir$",
                &cargo_config.target_dir.display().to_string(),
            );
            let lhs = lhs.replace(
                "$cargo_package_root$",
                &cargo_config.package_root.display().to_string(),
            );
            let lhs = lhs.replace("$executable$", &cargo_pkg_config.executable);

            // Extract file name from left-hand side
            let file = PathBuf::from(&lhs);
            let file_name = file.file_name().unwrap().to_string_lossy();

            // Expand right-hand side
            let rhs = rhs.replace("$file_name$", &file_name);

            output.push(format!("\"{}\" \"{}\"", lhs, rhs));
        } else {
            output.push(line.to_owned())
        }
    }

    Ok(output.join("\n"))
}

fn parse_mapping(line: &str) -> Option<(String, String)> {
    // The shortest valid line is `"a" "b"`, i.e. 7 characters long
    if line.len() < 7 || !line.starts_with('\"') || !line.ends_with('\"') {
        return None;
    }
    // Strip first and last quotation mark (")
    let line = &line[1..line.len() - 1];

    let end = line.find('\"');
    let begin = line.rfind('\"');
    if let (Some(end), Some(begin)) = (end, begin) {
        if end != begin {
            Some((line[..end].to_owned(), line[begin + 1..].to_owned()))
        } else {
            None
        }
    } else {
        None
    }
}

/// Constants used by the build system
///
const TEMPLATES_DIR: &str = "templates";

const XML_DISCLAIMER: &str = r#"
  <!--
    THIS PACKAGE MANIFEST FILE IS GENERATED BY THE BUILD PROCESS.

    Changes to this file will be lost when it is regenerated. To correct errors in this
    file, edit the respective input files (Cargo.toml, templates/AppxManifest.xml).
  -->"#;
