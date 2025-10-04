use chrono::Local;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const BACKUP_DIR: &str = "backup";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <backup|install>", args[0]);
        return Ok(());
    }

    match args[1].as_str() {
        "backup" => run_backup_mode(),
        "install" => run_install_mode(),
        _ => {
            eprintln!("Invalid argument: {}", args[1]);
            eprintln!("Usage: {} <backup|install>", args[0]);
            Ok(())
        }
    }
}

fn run_install_mode() -> Result<(), Box<dyn std::error::Error>> {
    if !is_adb_available() {
        eprintln!("Error: ADB command not found.");
        return Ok(());
    }

    if !is_device_connected() {
        eprintln!("Error : Device is disconnected. Please check device connection.");
        return Ok(());
    }

    let backup_root = Path::new(BACKUP_DIR);
    if !backup_root.exists() {
        return Err("Backup directory not found. Please run with 'backup' first.".into());
    }

    let entries: Vec<_> = fs::read_dir(backup_root)?
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .collect();

    if entries.is_empty() {
        eprintln!("No backups found.");
        return Ok(());
    }

    println!("Select backup to install:");
    for (i, entry) in entries.iter().enumerate() {
        println!("{}: {}", i + 1, entry.file_name().to_string_lossy());
    }

    let selected_backup = loop {
        print!("Enter number: ");
        io::stdout().flush()?;

        let mut choice = String::new();
        io::stdin().read_line(&mut choice)?;
        let index: usize = match choice.trim().parse() {
            Ok(n) if n >= 1 && n <= entries.len() => n,
            _ => {
                eprintln!("Invalid selection. Please enter a valid number.");
                continue;
            }
        };

        break entries[index - 1].path();
    };

    for entry in fs::read_dir(&selected_backup)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let apk_files: Vec<PathBuf> = fs::read_dir(&path)?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().map(|ext| ext == "apk").unwrap_or(false))
                .collect();

            if apk_files.is_empty() {
                eprintln!("No APKs found in {:?}", path);
                continue;
            }

            let output = if apk_files.len() == 1 {
                Command::new("adb")
                    .arg("install")
                    .arg(&apk_files[0])
                    .output()
            } else {
                let mut cmd = Command::new("adb");
                cmd.arg("install-multiple");
                for apk in &apk_files {
                    cmd.arg(apk);
                }
                cmd.output()
            };

            match output {
                Ok(output) if output.status.success() => {
                    println!("✓ Installed package from {:?}", path.file_name().unwrap());
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    eprintln!(
                        "✗ Failed to install from {:?}\nstdout: {}\nstderr: {}",
                        path.file_name().unwrap(),
                        stdout.trim(),
                        stderr.trim()
                    );
                }
                Err(e) => {
                    eprintln!("✗ Failed to execute adb install: {}", e);
                }
            }
        }
    }

    Ok(())
}

fn run_backup_mode() -> Result<(), Box<dyn std::error::Error>> {
    if !is_adb_available() {
        eprintln!("Error: ADB command not found.");
        return Ok(());
    }

    if !is_device_connected() {
        eprintln!("Error : Device is disconnected. Please check device connection.");
        return Ok(());
    }

    let backup_root = Path::new(BACKUP_DIR);
    if !backup_root.exists() {
        fs::create_dir(backup_root)?;
    }

    loop {
        println!("Select backup mode:");
        println!("1. New Backup");
        println!("2. Differential Backup");
        print!(": ");
        io::stdout().flush()?;

        let mut choice = String::new();
        io::stdin().read_line(&mut choice)?;

        match choice.trim() {
            "1" => return new_backup(backup_root),
            "2" => return differential_backup(backup_root),
            _ => {
                eprintln!("Invalid choice. Please enter 1 or 2.");
                continue;
            }
        }
    }
}

fn new_backup(backup_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("Enter backup name (or leave empty for timestamp):");
    let mut name = String::new();
    let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();

    loop {
        print!("> ");
        io::stdout().flush()?;
        name.clear();
        io::stdin().read_line(&mut name)?;
        let trimmed = name.trim();

        let folder_name = if trimmed.is_empty() {
            timestamp.clone()
        } else {
            let replaced = trimmed.replace("$date", &timestamp);
            if backup_root.join(&replaced).exists() {
                eprintln!("Folder already exists. Try another name.");
                continue;
            }
            replaced
        };

        return perform_backup(&backup_root.join(folder_name), None);
    }
}

fn differential_backup(backup_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let entries: Vec<_> = fs::read_dir(backup_root)?
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .collect();

    if entries.is_empty() {
        eprintln!("No backups found.");
        return Ok(());
    }

    println!("Select base backup:");
    for (i, entry) in entries.iter().enumerate() {
        println!("{}: {}", i + 1, entry.file_name().to_string_lossy());
    }

    print!("Enter number: ");
    io::stdout().flush()?;

    let mut choice = String::new();
    io::stdin().read_line(&mut choice)?;
    let index: usize = choice.trim().parse().unwrap_or(0);
    if index == 0 || index > entries.len() {
        eprintln!("Invalid selection.");
        return Ok(());
    }

    let base_backup = entries[index - 1].path();
    perform_backup(&base_backup, Some(&base_backup))
}

fn perform_backup(
    target_dir: &Path,
    base_backup: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(target_dir)?;
    let device_packages = get_third_party_packages()?;
    let base_packages = if let Some(base) = base_backup {
        fs::read_dir(base)?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect()
    } else {
        vec![]
    };

    let packages_to_backup: Vec<_> = device_packages
        .into_iter()
        .filter(|pkg| !base_packages.contains(pkg))
        .collect();

    if packages_to_backup.len() == 0 {
        println!("No package differences found for device.")
    }

    for (index, package) in packages_to_backup.iter().enumerate() {
        println!(
            "Backing up {} of {} ({})",
            index + 1,
            packages_to_backup.len(),
            package
        );
        match extract_apk(package, target_dir) {
            Ok(_) => println!("  ✓ Successful"),
            Err(e) => eprintln!("  ✗ Failed: {}", e),
        }
    }

    Ok(())
}

fn is_adb_available() -> bool {
    Command::new("adb").arg("version").output().is_ok()
}

fn is_device_connected() -> bool {
    if let Ok(output) = Command::new("adb").arg("devices").output() {
        let output_str = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = output_str.lines().collect();
        lines.len() > 1 && lines.iter().any(|line| line.contains("\tdevice"))
    } else {
        false
    }
}

fn get_third_party_packages() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let output = Command::new("adb")
        .args(&["shell", "pm", "list", "packages", "-3"])
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "adb command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let output_str = String::from_utf8(output.stdout)?;
    let packages: Vec<String> = output_str
        .lines()
        .filter_map(|line| {
            if line.starts_with("package:") {
                Some(line.replace("package:", "").trim().to_string())
            } else {
                None
            }
        })
        .collect();

    Ok(packages)
}

fn get_package_paths(package_name: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let output = Command::new("adb")
        .args(&["shell", "pm", "path", package_name])
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "Failed to get package path: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let output_str = String::from_utf8(output.stdout)?;
    let paths: Vec<String> = output_str
        .lines()
        .filter_map(|line| {
            if line.starts_with("package:") {
                Some(line.replace("package:", "").trim().to_string())
            } else {
                None
            }
        })
        .collect();

    if paths.is_empty() {
        return Err("Package path not found".into());
    }

    Ok(paths)
}

fn extract_apk(package_name: &str, work_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let apk_paths = get_package_paths(package_name)?;

    let package_dir = work_dir.join(package_name);
    if !package_dir.exists() {
        fs::create_dir_all(&package_dir)?;
    }

    println!("    Extracting {} APK file...", apk_paths.len());

    for (index, apk_path) in apk_paths.iter().enumerate() {
        let apk_filename = Path::new(apk_path)
            .file_name()
            .ok_or("Failed to get APK file name.")?
            .to_string_lossy();

        let local_apk_path = package_dir.join(&*apk_filename);

        let final_local_path = if local_apk_path.exists() {
            let stem = local_apk_path.file_stem().unwrap().to_string_lossy();
            let extension = local_apk_path
                .extension()
                .map(|ext| format!(".{}", ext.to_string_lossy()))
                .unwrap_or_default();
            package_dir.join(format!("{}_{}{}", stem, index + 1, extension))
        } else {
            local_apk_path
        };

        let output = Command::new("adb")
            .args(&[
                "pull",
                apk_path,
                final_local_path.to_string_lossy().as_ref(),
            ])
            .output()?;

        if !output.status.success() {
            eprintln!(
                "    Warning: Failed to extract {} : {}",
                apk_filename,
                String::from_utf8_lossy(&output.stderr)
            );
            continue;
        }

        if !final_local_path.exists() {
            eprintln!("    Warning: {} was not created", apk_filename);
            continue;
        }

        println!("      [{}/{}] {}", index + 1, apk_paths.len(), apk_filename);
    }

    Ok(())
}
