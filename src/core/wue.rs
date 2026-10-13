//! Windows User Experience (WUE) tweaks
//!
//! Replicates Rufus's wue.c functionality for bypassing Windows 11 requirements
//! and automating the initial Windows setup (OOBE).

use std::fs;
use std::path::Path;

/// Flags for Windows User Experience options matching Rufus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Generates a valid autounattend.xml content for Windows 10/11
pub fn generate_autounattend_xml(options: &WueOptions, username: &str, locale: &str) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    xml.push_str("<unattend xmlns=\"urn:schemas-microsoft-com:unattend\">\n");

    // windowsPE pass: LabConfig bypasses for TPM, SecureBoot, RAM
    xml.push_str("  <settings pass=\"windowsPE\">\n");
    xml.push_str("    <component name=\"Microsoft-Windows-Setup\" processorArchitecture=\"amd64\" publicKeyToken=\"31bf3856ad364e35\" language=\"neutral\" versionScope=\"nonSxS\" xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\">\n");

    if options.bypass_tpm_secureboot_ram {
        xml.push_str("      <RunSynchronous>\n");
        xml.push_str("        <RunSynchronousCommand wcm:action=\"add\">\n");
        xml.push_str("          <Order>1</Order>\n");
        xml.push_str("          <Path>reg.exe add \"HKLM\\SYSTEM\\Setup\\LabConfig\" /v BypassTPMCheck /t REG_DWORD /d 1 /f</Path>\n");
        xml.push_str("        </RunSynchronousCommand>\n");
        xml.push_str("        <RunSynchronousCommand wcm:action=\"add\">\n");
        xml.push_str("          <Order>2</Order>\n");
        xml.push_str("          <Path>reg.exe add \"HKLM\\SYSTEM\\Setup\\LabConfig\" /v BypassSecureBootCheck /t REG_DWORD /d 1 /f</Path>\n");
        xml.push_str("        </RunSynchronousCommand>\n");
        xml.push_str("        <RunSynchronousCommand wcm:action=\"add\">\n");
        xml.push_str("          <Order>3</Order>\n");
        xml.push_str("          <Path>reg.exe add \"HKLM\\SYSTEM\\Setup\\LabConfig\" /v BypassRAMCheck /t REG_DWORD /d 1 /f</Path>\n");
        xml.push_str("        </RunSynchronousCommand>\n");
        xml.push_str("        <RunSynchronousCommand wcm:action=\"add\">\n");
        xml.push_str("          <Order>4</Order>\n");
        xml.push_str("          <Path>reg.exe add \"HKLM\\SYSTEM\\Setup\\LabConfig\" /v BypassStorageCheck /t REG_DWORD /d 1 /f</Path>\n");
        xml.push_str("        </RunSynchronousCommand>\n");
        xml.push_str("        <RunSynchronousCommand wcm:action=\"add\">\n");
        xml.push_str("          <Order>5</Order>\n");
        xml.push_str("          <Path>reg.exe add \"HKLM\\SYSTEM\\Setup\\LabConfig\" /v BypassCPUCheck /t REG_DWORD /d 1 /f</Path>\n");
        xml.push_str("        </RunSynchronousCommand>\n");
        xml.push_str("      </RunSynchronous>\n");
    }

    if options.set_locale {
        xml.push_str("      <UserData>\n");
        xml.push_str("        <AcceptEula>true</AcceptEula>\n");
        xml.push_str("      </UserData>\n");
    }

    xml.push_str("    </component>\n");
    xml.push_str("  </settings>\n");

    // specialize pass: BitLocker disable & NRO bypass
    xml.push_str("  <settings pass=\"specialize\">\n");
    xml.push_str("    <component name=\"Microsoft-Windows-Deployment\" processorArchitecture=\"amd64\" publicKeyToken=\"31bf3856ad364e35\" language=\"neutral\" versionScope=\"nonSxS\" xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\">\n");
    xml.push_str("      <RunSynchronous>\n");
    let mut order = 1;

    if options.disable_bitlocker {
        xml.push_str(&format!(
            "        <RunSynchronousCommand wcm:action=\"add\">\n          <Order>{}</Order>\n          <Path>reg.exe add \"HKLM\\SYSTEM\\CurrentControlSet\\Control\\BitLocker\" /v PreventDeviceEncryption /t REG_DWORD /d 1 /f</Path>\n        </RunSynchronousCommand>\n",
            order
        ));
        order += 1;
    }

    if options.bypass_online_account {
        xml.push_str(&format!(
            "        <RunSynchronousCommand wcm:action=\"add\">\n          <Order>{}</Order>\n          <Path>reg.exe add \"HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\OOBE\" /v BypassNRO /t REG_DWORD /d 1 /f</Path>\n        </RunSynchronousCommand>\n",
            order
        ));
    }

    xml.push_str("      </RunSynchronous>\n");
    xml.push_str("    </component>\n");
    xml.push_str("  </settings>\n");

    // oobeSet pass: Privacy, local account, regional options
    xml.push_str("  <settings pass=\"oobeSet\">\n");
    xml.push_str("    <component name=\"Microsoft-Windows-Shell-Setup\" processorArchitecture=\"amd64\" publicKeyToken=\"31bf3856ad364e35\" language=\"neutral\" versionScope=\"nonSxS\" xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\">\n");

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

    if options.create_local_account && !username.trim().is_empty() {
        let safe_user = username.trim().replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        xml.push_str("      <UserAccounts>\n");
        xml.push_str("        <LocalAccounts>\n");
        xml.push_str("          <LocalAccount wcm:action=\"add\">\n");
        xml.push_str(&format!("            <Name>{}</Name>\n", safe_user));
        xml.push_str(&format!("            <DisplayName>{}</DisplayName>\n", safe_user));
        xml.push_str("            <Group>Administrators</Group>\n");
        xml.push_str("            <Password>\n");
        xml.push_str("              <Value></Value>\n");
        xml.push_str("              <PlainText>true</PlainText>\n");
        xml.push_str("            </Password>\n");
        xml.push_str("          </LocalAccount>\n");
        xml.push_str("        </LocalAccounts>\n");
        xml.push_str("      </UserAccounts>\n");
    }

    if options.set_locale && !locale.is_empty() {
        xml.push_str(&format!("      <InputLocale>{}</InputLocale>\n", locale));
        xml.push_str(&format!("      <SystemLocale>{}</SystemLocale>\n", locale));
        xml.push_str(&format!("      <UILanguage>{}</UILanguage>\n", locale));
        xml.push_str(&format!("      <UserLocale>{}</UserLocale>\n", locale));
    }

    xml.push_str("    </component>\n");
    xml.push_str("  </settings>\n");

    xml.push_str("</unattend>\n");
    xml
}

/// Injects the autounattend.xml into the target mount directory
pub fn install_wue_answer_file(
    target_mount_dir: &Path,
    options: &WueOptions,
    username: &str,
    locale: &str,
) -> anyhow::Result<()> {
    let xml = generate_autounattend_xml(options, username, locale);

    // Write to root autounattend.xml
    let root_xml_path = target_mount_dir.join("autounattend.xml");
    fs::write(&root_xml_path, &xml)?;

    // Also write to sources/autounattend.xml if sources directory exists
    let sources_dir = target_mount_dir.join("sources");
    if sources_dir.exists() {
        let _ = fs::write(sources_dir.join("autounattend.xml"), &xml);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wue_xml_generation() {
        let options = WueOptions::default();
        let xml = generate_autounattend_xml(&options, "RufusUser", "en-US");
        assert!(xml.contains("BypassTPMCheck"));
        assert!(xml.contains("BypassSecureBootCheck"));
        assert!(xml.contains("BypassRAMCheck"));
        assert!(xml.contains("BypassNRO"));
        assert!(xml.contains("RufusUser"));
        assert!(xml.contains("Administrators"));
        assert!(xml.contains("ProtectYourPC"));
    }
}
