//! Windows User Experience (WUE) tweaks
//!
//! Replicates Rufus's wue.c functionality for bypassing Windows 11 requirements
//! and automating the initial Windows setup (OOBE).

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

/// Flags for Windows User Experience options matching Rufus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WueOptions {
    /// Bypass TPM 2.0, SecureBoot, and 4GB+ RAM requirements
    pub bypass_tpm_secureboot_ram: bool,
    /// Bypass online Microsoft account requirement
    pub bypass_online_account: bool,
    /// Create a local account with a custom username
    pub create_local_account: bool,
    /// Set regional options / locale
    pub set_locale: bool,
    /// Disable BitLocker automatic device encryption
    pub disable_bitlocker: bool,
    /// Disable telemetry and privacy questions
    pub disable_data_collection: bool,
}

impl Default for WueOptions {
    fn default() -> Self {
        Self {
            bypass_tpm_secureboot_ram: true,
            bypass_online_account: true,
            create_local_account: true,
            set_locale: false,
            disable_bitlocker: true,
            disable_data_collection: true,
        }
    }
}

const COMPONENT_ATTRS: &str = "processorArchitecture=\"amd64\" publicKeyToken=\"31bf3856ad364e35\" language=\"neutral\" versionScope=\"nonSxS\" xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\"";

/// Characters Windows does not allow in a local account name.
const FORBIDDEN_USERNAME_CHARS: &str = "\"/\\[]:;|=,+*?<>@%";

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Checks a Windows local account name and returns it trimmed.
pub fn validate_username(name: &str) -> Result<String> {
    let name = name.trim();
    ensure!(!name.is_empty(), "the Windows account name is empty");
    ensure!(
        name.chars().count() <= 20,
        "the Windows account name is longer than 20 characters"
    );
    ensure!(
        !name.ends_with('.'),
        "the Windows account name cannot end with a period"
    );
    ensure!(
        !name
            .chars()
            .any(|c| c.is_control() || FORBIDDEN_USERNAME_CHARS.contains(c)),
        "the Windows account name contains characters Windows does not allow"
    );
    Ok(name.to_string())
}

fn validate_locale(locale: &str) -> Result<&str> {
    ensure!(
        (2..=35).contains(&locale.len()) && locale.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "invalid locale '{}'",
        locale
    );
    Ok(locale)
}

/// Derives a Windows locale name (`en-US`) from the POSIX locale environment.
pub fn detect_locale() -> String {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    locale_from_posix(&raw)
}

fn locale_from_posix(raw: &str) -> String {
    let base = raw.split(['.', '@']).next().unwrap_or("");
    let mut parts = base.split('_');
    match (parts.next(), parts.next()) {
        (Some(lang), Some(region)) if lang.len() >= 2 && region.len() >= 2 && lang != "C" => {
            format!("{}-{}", lang.to_ascii_lowercase(), region.to_ascii_uppercase())
        }
        (Some(lang), None) if lang.len() == 2 && lang.chars().all(|c| c.is_ascii_lowercase()) => {
            format!("{}-{}", lang, lang.to_ascii_uppercase())
        }
        _ => "en-US".to_string(),
    }
}

fn run_command(xml: &mut String, order: u32, command: &str) {
    let _ = write!(
        xml,
        "        <RunSynchronousCommand wcm:action=\"add\">\n          <Order>{}</Order>\n          <Path>{}</Path>\n        </RunSynchronousCommand>\n",
        order,
        xml_escape(command)
    );
}

fn reg_add(key: &str, value: &str) -> String {
    format!("reg.exe add \"{}\" /v {} /t REG_DWORD /d 1 /f", key, value)
}

/// Generates a valid autounattend.xml content for Windows 10/11
pub fn generate_autounattend_xml(options: &WueOptions, username: &str, locale: &str) -> Result<String> {
    let user = if options.create_local_account {
        Some(validate_username(username)?)
    } else {
        None
    };
    let locale = if options.set_locale {
        Some(validate_locale(locale)?)
    } else {
        None
    };

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    xml.push_str("<unattend xmlns=\"urn:schemas-microsoft-com:unattend\">\n");

    // windowsPE pass: LabConfig bypasses for TPM, SecureBoot, RAM
    if options.bypass_tpm_secureboot_ram {
        const LAB: &str = "HKLM\\SYSTEM\\Setup\\LabConfig";
        xml.push_str("  <settings pass=\"windowsPE\">\n");
        let _ = writeln!(
            xml,
            "    <component name=\"Microsoft-Windows-Setup\" {}>",
            COMPONENT_ATTRS
        );
        xml.push_str("      <RunSynchronous>\n");
        for (i, value) in [
            "BypassTPMCheck",
            "BypassSecureBootCheck",
            "BypassRAMCheck",
            "BypassStorageCheck",
            "BypassCPUCheck",
        ]
        .iter()
        .enumerate()
        {
            run_command(&mut xml, i as u32 + 1, &reg_add(LAB, value));
        }
        xml.push_str("      </RunSynchronous>\n    </component>\n  </settings>\n");
    }

    // specialize pass: BitLocker disable & NRO bypass
    let mut specialize = Vec::new();
    if options.disable_bitlocker {
        specialize.push(reg_add(
            "HKLM\\SYSTEM\\CurrentControlSet\\Control\\BitLocker",
            "PreventDeviceEncryption",
        ));
    }
    if options.bypass_online_account {
        specialize.push(reg_add(
            "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\OOBE",
            "BypassNRO",
        ));
    }
    if !specialize.is_empty() {
        xml.push_str("  <settings pass=\"specialize\">\n");
        let _ = writeln!(
            xml,
            "    <component name=\"Microsoft-Windows-Deployment\" {}>",
            COMPONENT_ATTRS
        );
        xml.push_str("      <RunSynchronous>\n");
        for (i, cmd) in specialize.iter().enumerate() {
            run_command(&mut xml, i as u32 + 1, cmd);
        }
        xml.push_str("      </RunSynchronous>\n    </component>\n  </settings>\n");
    }

    // oobeSystem pass: privacy, local account, regional options
    xml.push_str("  <settings pass=\"oobeSystem\">\n");
    if let Some(locale) = locale {
        let l = xml_escape(locale);
        let _ = writeln!(
            xml,
            "    <component name=\"Microsoft-Windows-International-Core\" {}>",
            COMPONENT_ATTRS
        );
        let _ = write!(
            xml,
            "      <InputLocale>{l}</InputLocale>\n      <SystemLocale>{l}</SystemLocale>\n      <UILanguage>{l}</UILanguage>\n      <UserLocale>{l}</UserLocale>\n"
        );
        xml.push_str("    </component>\n");
    }

    let _ = writeln!(
        xml,
        "    <component name=\"Microsoft-Windows-Shell-Setup\" {}>",
        COMPONENT_ATTRS
    );
    xml.push_str("      <OOBE>\n");
    if options.disable_data_collection {
        xml.push_str("        <ProtectYourPC>3</ProtectYourPC>\n");
        xml.push_str("        <HideEULAPage>true</HideEULAPage>\n");
        xml.push_str("        <HideWirelessSetupInOOBE>false</HideWirelessSetupInOOBE>\n");
    }
    if options.bypass_online_account {
        xml.push_str("        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>\n");
    }
    xml.push_str("      </OOBE>\n");

    if let Some(user) = user {
        let user = xml_escape(&user);
        xml.push_str("      <UserAccounts>\n        <LocalAccounts>\n          <LocalAccount wcm:action=\"add\">\n");
        let _ = writeln!(xml, "            <Name>{}</Name>", user);
        let _ = writeln!(xml, "            <DisplayName>{}</DisplayName>", user);
        xml.push_str("            <Group>Administrators</Group>\n");
        xml.push_str("            <Password>\n              <Value></Value>\n              <PlainText>true</PlainText>\n            </Password>\n");
        xml.push_str("          </LocalAccount>\n        </LocalAccounts>\n      </UserAccounts>\n");
    }
    xml.push_str("    </component>\n  </settings>\n</unattend>\n");
    Ok(xml)
}

/// Writes the answer file into the target volume (root, plus `sources/` when present).
pub fn write_answer_file(target_mount_dir: &Path, xml: &str) -> Result<()> {
    fs::write(target_mount_dir.join("autounattend.xml"), xml).context("cannot write autounattend.xml")?;

    let sources_dir = target_mount_dir.join("sources");
    if sources_dir.is_dir() {
        fs::write(sources_dir.join("autounattend.xml"), xml).context("cannot write sources/autounattend.xml")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wue_xml_generation() {
        let options = WueOptions::default();
        let xml = generate_autounattend_xml(&options, "RufusUser", "en-US").unwrap();
        for needle in [
            "BypassTPMCheck",
            "BypassSecureBootCheck",
            "BypassRAMCheck",
            "BypassNRO",
            "PreventDeviceEncryption",
            "RufusUser",
            "Administrators",
            "ProtectYourPC",
            "pass=\"oobeSystem\"",
        ] {
            assert!(xml.contains(needle), "missing {needle}");
        }
        assert!(!xml.contains("oobeSet\""), "the OOBE pass is called oobeSystem");
    }

    #[test]
    fn disabled_options_leave_no_empty_sections() {
        let options = WueOptions {
            bypass_tpm_secureboot_ram: false,
            bypass_online_account: false,
            create_local_account: false,
            set_locale: false,
            disable_bitlocker: false,
            disable_data_collection: false,
        };
        let xml = generate_autounattend_xml(&options, "", "").unwrap();
        assert!(!xml.contains("RunSynchronous"));
        assert!(!xml.contains("UserAccounts"));
    }

    #[test]
    fn locale_goes_into_the_international_component() {
        let options = WueOptions {
            set_locale: true,
            ..WueOptions::default()
        };
        let xml = generate_autounattend_xml(&options, "user", "de-DE").unwrap();
        assert!(xml.contains("Microsoft-Windows-International-Core"));
        assert!(xml.contains("<UILanguage>de-DE</UILanguage>"));
        assert!(generate_autounattend_xml(&options, "user", "<x/>").is_err());
    }

    #[test]
    fn hostile_usernames_are_rejected_not_escaped_into_the_file() {
        let options = WueOptions::default();
        assert!(generate_autounattend_xml(&options, "a<b>", "en-US").is_err());
        assert!(generate_autounattend_xml(&options, "   ", "en-US").is_err());
        assert!(generate_autounattend_xml(&options, "averyveryverylongusername", "en-US").is_err());
        assert_eq!(xml_escape("a&b\"'"), "a&amp;b&quot;&apos;");
    }

    #[test]
    fn posix_locales_map_to_windows_names() {
        assert_eq!(locale_from_posix("en_US.UTF-8"), "en-US");
        assert_eq!(locale_from_posix("de_DE@euro"), "de-DE");
        assert_eq!(locale_from_posix("fr"), "fr-FR");
        assert_eq!(locale_from_posix("C"), "en-US");
        assert_eq!(locale_from_posix(""), "en-US");
    }
}
