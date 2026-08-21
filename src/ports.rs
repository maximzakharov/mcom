use std::fs;
use std::path::Path;

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortInfo {
    pub path: String,
    pub label: String,
    pub is_usb: bool,
}

/// Ports that always exist on macOS and are never the board you are looking for.
const MACOS_NOISE: &[&str] = &[
    "cu.Bluetooth-Incoming-Port",
    "tty.Bluetooth-Incoming-Port",
    "cu.debug-console",
    "tty.debug-console",
    "cu.wlan-debug",
    "tty.wlan-debug",
];

pub fn looks_like_usb(path: &str) -> bool {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    const HINTS: &[&str] = &[
        "usbmodem",
        "usbserial",
        "wchusbserial",
        "SLAB_USBtoUART",
        "ttyUSB",
        "ttyACM",
        "usbmodemserial",
    ];
    HINTS.iter().any(|h| leaf.contains(h))
}

fn is_noise(path: &str) -> bool {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    MACOS_NOISE.contains(&leaf)
}

/// On macOS `/dev/tty.*` blocks on open until DCD is asserted, which is the
/// classic "the terminal just hangs" trap. The `/dev/cu.*` twin does not.
pub fn prefer_callout(path: &str) -> Option<String> {
    let leaf = path.rsplit('/').next()?;
    let rest = leaf.strip_prefix("tty.")?;
    let cu = format!("/dev/cu.{rest}");
    Path::new(&cu).exists().then_some(cu)
}

pub fn list() -> Vec<PortInfo> {
    let mut found: Vec<PortInfo> = Vec::new();

    if let Ok(ports) = serialport::available_ports() {
        for p in ports {
            let label = match &p.port_type {
                serialport::SerialPortType::UsbPort(u) => {
                    let mut parts = Vec::new();
                    if let Some(m) = &u.manufacturer {
                        parts.push(m.clone());
                    }
                    if let Some(pr) = &u.product {
                        parts.push(pr.clone());
                    }
                    if parts.is_empty() {
                        format!("USB {:04x}:{:04x}", u.vid, u.pid)
                    } else {
                        parts.join(" ")
                    }
                }
                serialport::SerialPortType::BluetoothPort => "Bluetooth".into(),
                serialport::SerialPortType::PciPort => "PCI".into(),
                serialport::SerialPortType::Unknown => String::new(),
            };
            let is_usb = matches!(p.port_type, serialport::SerialPortType::UsbPort(_))
                || looks_like_usb(&p.port_name);
            push_unique(
                &mut found,
                PortInfo {
                    path: p.port_name,
                    label,
                    is_usb,
                },
            );
        }
    }

    for p in scan_dev() {
        push_unique(&mut found, p);
    }

    found.retain(|p| !is_noise(&p.path));
    if cfg!(target_os = "macos") {
        // Keep the callout device only; the tty twin is the same hardware.
        found.retain(|p| !p.path.starts_with("/dev/tty."));
    }
    found.sort_by(|a, b| b.is_usb.cmp(&a.is_usb).then(a.path.cmp(&b.path)));
    found
}

fn push_unique(found: &mut Vec<PortInfo>, port: PortInfo) {
    if let Some(existing) = found.iter_mut().find(|p| p.path == port.path) {
        if existing.label.is_empty() {
            existing.label = port.label;
        }
        existing.is_usb |= port.is_usb;
        return;
    }
    found.push(port);
}

#[cfg(target_os = "macos")]
fn scan_dev() -> Vec<PortInfo> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/dev") else {
        return out;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("cu.") {
            continue;
        }
        let path = format!("/dev/{name}");
        let is_usb = looks_like_usb(&path);
        out.push(PortInfo {
            path,
            label: String::new(),
            is_usb,
        });
    }
    out
}

#[cfg(target_os = "linux")]
fn scan_dev() -> Vec<PortInfo> {
    let mut out = Vec::new();

    // /dev/serial/by-id carries human-readable names; resolve them to /dev/ttyX.
    if let Ok(entries) = fs::read_dir("/dev/serial/by-id") {
        for e in entries.flatten() {
            let Ok(target) = fs::canonicalize(e.path()) else {
                continue;
            };
            out.push(PortInfo {
                path: target.to_string_lossy().into_owned(),
                label: e.file_name().to_string_lossy().into_owned(),
                is_usb: true,
            });
        }
    }

    if let Ok(entries) = fs::read_dir("/sys/class/tty") {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !e.path().join("device").exists() {
                continue;
            }
            let path = format!("/dev/{name}");
            if !Path::new(&path).exists() {
                continue;
            }
            let is_usb = looks_like_usb(&path);
            out.push(PortInfo {
                path,
                label: String::new(),
                is_usb,
            });
        }
    }
    out
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn scan_dev() -> Vec<PortInfo> {
    Vec::new()
}

/// A stable name for the device behind a port. Port names are not identity:
/// `/dev/ttyACM0` is whichever board enumerated first, and a reset can hand the
/// name to something else entirely. Vendor, product and serial do identify it.
pub fn device_identity(path: &str) -> Option<String> {
    let path = fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    identity_of(&identities(), &path)
}

/// The port that device currently sits on, if it is plugged in at all.
pub fn path_for_identity(identity: &str) -> Option<String> {
    path_of(&identities(), identity)
}

fn identity_of(entries: &[(String, String)], path: &str) -> Option<String> {
    entries
        .iter()
        .find(|(_, p)| p == path)
        .map(|(id, _)| id.clone())
}

fn path_of(entries: &[(String, String)], identity: &str) -> Option<String> {
    entries
        .iter()
        .find(|(id, _)| id == identity)
        .map(|(_, p)| p.clone())
}

#[cfg(target_os = "linux")]
fn identities() -> Vec<(String, String)> {
    // The by-id name is exactly vendor, product and serial, and it survives a
    // ttyACM0 -> ttyACM1 renumbering.
    let Ok(entries) = fs::read_dir("/dev/serial/by-id") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let target = fs::canonicalize(e.path()).ok()?;
            Some((
                e.file_name().to_string_lossy().into_owned(),
                target.to_string_lossy().into_owned(),
            ))
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn identities() -> Vec<(String, String)> {
    let Ok(ports) = serialport::available_ports() else {
        return Vec::new();
    };
    ports
        .into_iter()
        .filter_map(|p| match p.port_type {
            // Without a serial number two identical boards are indistinguishable,
            // so such a device is treated as having no identity at all.
            serialport::SerialPortType::UsbPort(u) => {
                let serial = u.serial_number?;
                Some((
                    format!("usb-{:04x}:{:04x}-{serial}", u.vid, u.pid),
                    p.port_name,
                ))
            }
            _ => None,
        })
        .collect()
}

pub fn print_list() {
    let ports = list();
    if ports.is_empty() {
        println!("no serial ports found");
        return;
    }
    let width = ports.iter().map(|p| p.path.len()).max().unwrap_or(0);
    for p in &ports {
        let tag = if p.is_usb { "usb" } else { "   " };
        if p.label.is_empty() {
            println!("{tag}  {:width$}", p.path);
        } else {
            println!("{tag}  {:width$}  {}", p.path, p.label);
        }
    }
}

/// Picks the port to open: the one given on the command line, the only USB port
/// present, or an interactive choice.
pub fn choose(requested: Option<&str>) -> Result<String> {
    if let Some(p) = requested {
        if let Some(cu) = prefer_callout(p) {
            eprintln!("note: using {cu} instead of {p} (tty devices block until DCD)");
            return Ok(cu);
        }
        return Ok(p.to_string());
    }

    let ports = list();
    let usb: Vec<&PortInfo> = ports.iter().filter(|p| p.is_usb).collect();

    // Only USB ports are auto-selected: built-in and Bluetooth serial devices
    // are almost never what you meant, and opening one wastes your time.
    match usb.len() {
        1 => return Ok(usb[0].path.clone()),
        0 if ports.is_empty() => {
            bail!("no serial ports found; plug in a board or pass a path explicitly")
        }
        0 => {
            let names: Vec<&str> = ports.iter().map(|p| p.path.as_str()).collect();
            bail!(
                "no USB serial port found; pass one of these explicitly: {}",
                names.join(", ")
            )
        }
        _ => {}
    }

    println!("Multiple ports found:");
    for (i, p) in usb.iter().enumerate() {
        if p.label.is_empty() {
            println!("  {}) {}", i + 1, p.path);
        } else {
            println!("  {}) {}  {}", i + 1, p.path, p.label);
        }
    }
    print!("Select [1-{}]: ", usb.len());
    use std::io::Write;
    std::io::stdout().flush()?;

    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let idx: usize = answer
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("not a number: {:?}", answer.trim()))?;
    if idx < 1 || idx > usb.len() {
        bail!("choice out of range");
    }
    Ok(usb[idx - 1].path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_usb_port_names() {
        assert!(looks_like_usb("/dev/cu.usbmodem1101"));
        assert!(looks_like_usb("/dev/ttyACM0"));
        assert!(looks_like_usb("/dev/ttyUSB0"));
        assert!(!looks_like_usb("/dev/cu.Bluetooth-Incoming-Port"));
        assert!(!looks_like_usb("/dev/ttyS0"));
    }

    #[test]
    fn filters_the_usual_macos_noise() {
        assert!(is_noise("/dev/cu.Bluetooth-Incoming-Port"));
        assert!(is_noise("/dev/cu.debug-console"));
        assert!(!is_noise("/dev/cu.usbmodem1101"));
    }

    #[test]
    fn identity_maps_both_ways() {
        let entries = vec![
            (
                "usb-STMicroelectronics_STM32_Device_344F33483133-if00".to_string(),
                "/dev/ttyACM0".to_string(),
            ),
            (
                "usb-Black_Magic_Debug_Black_Magic_Probe-if00".to_string(),
                "/dev/ttyACM1".to_string(),
            ),
        ];
        let stm = identity_of(&entries, "/dev/ttyACM0").unwrap();
        assert!(stm.contains("STM32"));
        assert_eq!(path_of(&entries, &stm).as_deref(), Some("/dev/ttyACM0"));

        // The board is unplugged: its identity resolves to nothing, and in
        // particular not to the debug probe that is still connected.
        let gone: Vec<(String, String)> = entries[1..].to_vec();
        assert_eq!(path_of(&gone, &stm), None);
        assert_eq!(identity_of(&gone, "/dev/ttyACM0"), None);
    }

    #[test]
    fn merges_duplicate_paths_keeping_the_better_label() {
        let mut v = Vec::new();
        push_unique(
            &mut v,
            PortInfo {
                path: "/dev/x".into(),
                label: String::new(),
                is_usb: false,
            },
        );
        push_unique(
            &mut v,
            PortInfo {
                path: "/dev/x".into(),
                label: "Espressif".into(),
                is_usb: true,
            },
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].label, "Espressif");
        assert!(v[0].is_usb);
    }
}
