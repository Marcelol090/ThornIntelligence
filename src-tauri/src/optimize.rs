use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationResult {
    pub drive: String,
    pub executed: bool,
    pub output: String,
}

fn validated_drive(input: &str) -> Result<char, String> {
    let value = input.trim().trim_end_matches(':');
    if value.len() != 1 {
        return Err("Informe somente a letra da unidade, por exemplo C.".into());
    }
    let letter = value.chars().next().unwrap().to_ascii_uppercase();
    if !letter.is_ascii_alphabetic() {
        return Err("A unidade deve ser uma letra entre A e Z.".into());
    }
    Ok(letter)
}

#[cfg(target_os = "windows")]
pub fn run(drive: String, execute: bool) -> Result<OptimizationResult, String> {
    use std::process::Command;
    let letter = validated_drive(&drive)?;

    // The OS determines the appropriate operation for SSD/HDD/tiered volumes.
    // Never force -Defrag against a solid-state disk.
    // execute=false is always a read-only analysis.
    let mode = if execute { "" } else { "-Analyze " };
    let command = format!(
        "$ErrorActionPreference='Stop'; Optimize-Volume -DriveLetter {letter} {mode}-Verbose 4>&1 | Out-String -Width 220"
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .map_err(|err| format!("Não foi possível iniciar o PowerShell: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(format!(
            "Optimize-Volume falhou. Verifique a unidade e as permissões de administrador. {stderr} {stdout}"
        ));
    }
    Ok(OptimizationResult {
        drive: letter.to_string(),
        executed: execute,
        output: if stdout.is_empty() {
            "Comando concluído sem detalhes adicionais.".to_owned()
        } else {
            stdout
        },
    })
}

#[cfg(not(target_os = "windows"))]
pub fn run(drive: String, _execute: bool) -> Result<OptimizationResult, String> {
    let _ = validated_drive(&drive)?;
    Err("O adaptador de otimização é exclusivo do Windows nesta versão. Nenhuma operação foi executada.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_letter_is_strictly_validated() {
        assert_eq!(validated_drive("c:").unwrap(), 'C');
        assert!(validated_drive("C;Remove-Item").is_err());
        assert!(validated_drive("C:\\").is_err());
    }
}
