use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskHealth {
    pub drive: String,
    pub model: Option<String>,
    pub bus_type: Option<String>,
    pub disk_number: Option<u32>,
    pub health_status: Option<String>,
    pub operational_status: Option<String>,
    pub size_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub temperature_c: Option<u32>,
    pub wear_percent: Option<u32>,
    pub read_errors_uncorrected: Option<u64>,
    pub write_errors_uncorrected: Option<u64>,
    pub power_on_hours: Option<u64>,
    pub reliability_available: bool,
}

#[cfg(target_os = "windows")]
pub fn read(drive: String) -> Result<DiskHealth, String> {
    use std::process::Command;

    let letter = crate::optimize::validated_drive(&drive)?;
    // Read-only Windows Storage cmdlets. Disk and volume are resolved through
    // the partition; do not infer media type from SATA/NVMe bus identifiers.
    // Reliability counters can legitimately be unavailable on a device.
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$partition = Get-Partition -DriveLetter {letter} -ErrorAction Stop
$disk = $partition | Get-Disk -ErrorAction Stop
$volume = Get-Volume -DriveLetter {letter} -ErrorAction Stop
$counter = $null
try {{ $counter = $disk | Get-StorageReliabilityCounter -ErrorAction Stop }} catch {{ }}
[pscustomobject]@{{
    drive = '{letter}'
    model = [string]$disk.FriendlyName
    busType = [string]$disk.BusType
    diskNumber = [uint32]$disk.Number
    healthStatus = [string]$disk.HealthStatus
    operationalStatus = (@($disk.OperationalStatus) -join ', ')
    sizeBytes = [uint64]$volume.Size
    freeBytes = [uint64]$volume.SizeRemaining
    temperatureC = if ($null -ne $counter) {{ $counter.Temperature }} else {{ $null }}
    wearPercent = if ($null -ne $counter) {{ $counter.Wear }} else {{ $null }}
    readErrorsUncorrected = if ($null -ne $counter) {{ $counter.ReadErrorsUncorrected }} else {{ $null }}
    writeErrorsUncorrected = if ($null -ne $counter) {{ $counter.WriteErrorsUncorrected }} else {{ $null }}
    powerOnHours = if ($null -ne $counter) {{ $counter.PowerOnHours }} else {{ $null }}
    reliabilityAvailable = ($null -ne $counter)
}} | ConvertTo-Json -Compress -Depth 4
"#
    );
    let result = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|err| format!("Não foi possível consultar o Windows Storage: {err}"))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(format!(
            "Não foi possível consultar a unidade {letter}: {}",
            stderr.trim()
        ));
    }
    let text = String::from_utf8_lossy(&result.stdout);
    let health: DiskHealth = serde_json::from_str(text.trim())
        .map_err(|err| format!("O Windows devolveu um diagnóstico inválido: {err}"))?;
    if health.drive != letter.to_string() {
        return Err("O diagnóstico não corresponde à unidade solicitada.".into());
    }
    Ok(health)
}

#[cfg(not(target_os = "windows"))]
pub fn read(_drive: String) -> Result<DiskHealth, String> {
    Err("Leitura de saúde de discos disponível apenas no Windows nesta versão.".into())
}
