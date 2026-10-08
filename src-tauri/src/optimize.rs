use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const ANALYSIS_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizationResult {
    pub drive: String,
    pub executed: bool,
    pub output: String,
}

#[derive(Default)]
struct VolumeGate {
    generation: u64,
    analyzing: bool,
    analyzed_at: Option<Instant>,
    optimizing: bool,
}

/// An in-memory, per-volume gate. The renderer cannot forge a prior analysis
/// by calling the execution command directly. Grants expire and are single-use.
#[derive(Default)]
pub struct OptimizationGate {
    states: Mutex<HashMap<char, VolumeGate>>,
}

impl OptimizationGate {
    fn begin_analysis(&self, drive: char) -> Result<u64, String> {
        let mut states = self.states.lock().map_err(|_| "Estado de segurança indisponível.")?;
        let state = states.entry(drive).or_default();
        if state.analyzing || state.optimizing {
            return Err("Já existe uma operação em andamento nesta unidade.".into());
        }
        state.generation = state.generation.wrapping_add(1);
        state.analyzing = true;
        state.analyzed_at = None; // Revoke previous approval before a new attempt.
        Ok(state.generation)
    }

    fn finish_analysis(&self, drive: char, generation: u64, succeeded: bool) -> Result<(), String> {
        let mut states = self.states.lock().map_err(|_| "Estado de segurança indisponível.")?;
        let state = states.get_mut(&drive).ok_or("Análise não registrada.")?;
        if state.generation != generation || !state.analyzing {
            return Err("A análise foi substituída por outra operação.".into());
        }
        state.analyzing = false;
        state.analyzed_at = succeeded.then(Instant::now);
        Ok(())
    }

    fn begin_optimization(&self, drive: char) -> Result<(), String> {
        let mut states = self.states.lock().map_err(|_| "Estado de segurança indisponível.")?;
        let state = states.entry(drive).or_default();
        if state.analyzing || state.optimizing {
            return Err("Já existe uma operação em andamento nesta unidade.".into());
        }
        let approved = state.analyzed_at.take()
            .ok_or("É obrigatório analisar esta unidade antes de otimizá-la.")?;
        if approved.elapsed() > ANALYSIS_TTL {
            return Err("A análise expirou (5 minutos). Analise a unidade novamente.".into());
        }
        state.optimizing = true;
        Ok(())
    }

    fn finish_optimization(&self, drive: char) -> Result<(), String> {
        let mut states = self.states.lock().map_err(|_| "Estado de segurança indisponível.")?;
        let state = states.get_mut(&drive).ok_or("Operação não registrada.")?;
        state.optimizing = false;
        Ok(())
    }
}

pub(crate) fn validated_drive(input: &str) -> Result<char, String> {
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
fn windows_command(letter: char, execute: bool) -> Result<OptimizationResult, String> {
    use std::process::Command;
    // With no explicit operation, Windows selects HDD defrag, SSD ReTRIM,
    // tier optimization, etc. Never force -Defrag against SSD.
    let mode = if execute { "" } else { "-Analyze " };
    let command = format!(
        "$ErrorActionPreference='Stop'; Optimize-Volume -DriveLetter {letter} {mode}-ErrorAction Stop -Verbose 4>&1 | Out-String -Width 220"
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .output()
        .map_err(|err| format!("Não foi possível iniciar o PowerShell: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(format!(
            "Optimize-Volume falhou. Verifique a unidade e as permissões. {stderr} {stdout}"
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

#[cfg(target_os = "windows")]
pub fn run(drive: String, execute: bool, gate: &OptimizationGate) -> Result<OptimizationResult, String> {
    let letter = validated_drive(&drive)?;
    if execute {
        gate.begin_optimization(letter)?;
        let result = windows_command(letter, true);
        gate.finish_optimization(letter)?;
        result
    } else {
        let generation = gate.begin_analysis(letter)?;
        let result = windows_command(letter, false);
        gate.finish_analysis(letter, generation, result.is_ok())?;
        result
    }
}

#[cfg(not(target_os = "windows"))]
pub fn run(drive: String, _execute: bool, _gate: &OptimizationGate) -> Result<OptimizationResult, String> {
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

    #[test]
    fn execution_requires_successful_analysis_on_same_drive_and_is_single_use() {
        let gate = OptimizationGate::default();
        assert!(gate.begin_optimization('C').is_err());
        let generation = gate.begin_analysis('C').unwrap();
        assert!(gate.begin_optimization('C').is_err()); // Still analyzing.
        gate.finish_analysis('C', generation, true).unwrap();
        assert!(gate.begin_optimization('D').is_err());
        gate.begin_optimization('C').unwrap();
        assert!(gate.begin_optimization('C').is_err());
        gate.finish_optimization('C').unwrap();
        assert!(gate.begin_optimization('C').is_err()); // Grant consumed.
    }

    #[test]
    fn failed_or_expired_analysis_never_authorizes_execution() {
        let gate = OptimizationGate::default();
        let failed = gate.begin_analysis('C').unwrap();
        gate.finish_analysis('C', failed, false).unwrap();
        assert!(gate.begin_optimization('C').is_err());

        let ok = gate.begin_analysis('C').unwrap();
        gate.finish_analysis('C', ok, true).unwrap();
        {
            let mut states = gate.states.lock().unwrap();
            states.get_mut(&'C').unwrap().analyzed_at =
                Some(Instant::now() - ANALYSIS_TTL - Duration::from_secs(1));
        }
        assert!(gate.begin_optimization('C').is_err());
    }

    #[test]
    fn concurrent_analysis_is_rejected_and_new_analysis_revokes_old_grant() {
        let gate = OptimizationGate::default();
        let first = gate.begin_analysis('C').unwrap();
        assert!(gate.begin_analysis('C').is_err());
        gate.finish_analysis('C', first, true).unwrap();
        let second = gate.begin_analysis('C').unwrap();
        assert!(gate.begin_optimization('C').is_err());
        gate.finish_analysis('C', second, false).unwrap();
        assert!(gate.begin_optimization('C').is_err());
    }
}
