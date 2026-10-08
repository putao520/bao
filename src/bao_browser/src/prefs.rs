// Pref override surface (user ruling 2026-10-08 ruling A, e150 campaign
// root finding). Lives OUTSIDE the `webdriver` feature gate: the durable
// application point is `BrowserRuntime::new`'s `ServoBuilder::preferences`
// construction, which is core — `Servo::new` ends with
// `prefs::set(builder.preferences.unwrap_or_default())`, so any purely
// global application (the old CLI path) is wiped by the reset.
use servo::Preferences;

/// Apply raw `--pref=K=V` overrides onto the process-global servo prefs.
///
/// coercion: `true`/`false` → bool, numeric → f64, everything else stays a
/// string. Fail-closed: a bad override aborts the launch instead of silently
/// running with missing test prefs.
pub fn apply_pref_overrides(overrides: &[(String, String)]) -> Result<(), String> {
    if overrides.is_empty() {
        return Ok(());
    }
    let merged = apply_pref_overrides_to(servo::prefs::get().clone(), overrides)?;
    servo::prefs::set(merged);
    Ok(())
}

/// Apply raw `--pref=K=V` overrides onto a `Preferences` value and return the
/// merged result (same coercion as [`apply_pref_overrides`]; schema-mismatch
/// names are ignored by serde and land as no-ops). Shared core for the two
/// consumers that must not drift: the global pre-launch application above
/// (fail-closed early validation) and the `ServoBuilder::preferences`
/// construction in `BrowserRuntime::new` — a builder-side application is the
/// only form of override that survives `Servo::new`'s prefs reset.
pub fn apply_pref_overrides_to(
    mut preferences: Preferences,
    overrides: &[(String, String)],
) -> Result<Preferences, String> {
    if overrides.is_empty() {
        return Ok(preferences);
    }
    let mut value = serde_json::to_value(&preferences)
        .map_err(|e| format!("serializing builder prefs failed: {e}"))?;
    let serde_json::Value::Object(ref mut map) = value else {
        return Err("prefs did not serialize to a JSON object".into());
    };
    for (key, raw) in overrides {
        let coerced = match raw.as_str() {
            "true" => serde_json::Value::Bool(true),
            "false" => serde_json::Value::Bool(false),
            other => other
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
                .unwrap_or_else(|| serde_json::Value::String(other.to_string())),
        };
        map.insert(key.clone(), coerced);
    }
    preferences = serde_json::from_value(value)
        .map_err(|e| format!("pref overrides do not match the Preferences schema: {e}"))?;
    Ok(preferences)
}

/// Load a wpt `--prefs-file` (JSON object of pref → raw value) into the same
/// override list `apply_pref_overrides` consumes. Fail-closed on unreadable
/// or non-object files.
pub fn load_prefs_file(path: &str) -> Result<Vec<(String, String)>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("reading prefs file {path} failed: {e}"))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parsing prefs file {path} failed: {e}"))?;
    let serde_json::Value::Object(map) = parsed else {
        return Err(format!("prefs file {path} is not a JSON object"));
    };
    Ok(map
        .into_iter()
        .map(|(key, value)| {
            let raw = match value {
                serde_json::Value::Bool(true) => "true".to_string(),
                serde_json::Value::Bool(false) => "false".to_string(),
                serde_json::Value::Number(number) => number.to_string(),
                serde_json::Value::String(string) => string,
                other => other.to_string(),
            };
            (key, raw)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_override_flips_pref() {
        let mut prefs = Preferences::default();
        assert!(!prefs.dom_exec_command_enabled);
        prefs = apply_pref_overrides_to(
            prefs,
            &[("dom_exec_command_enabled".to_string(), "true".to_string())],
        )
        .expect("bool override must apply");
        assert!(prefs.dom_exec_command_enabled);
    }

    #[test]
    fn schema_mismatch_name_is_noop() {
        let prefs = apply_pref_overrides_to(
            Preferences::default(),
            &[("no_such_pref_name".to_string(), "true".to_string())],
        )
        .expect("unknown pref name is a serde no-op, not an error");
        assert!(!prefs.dom_exec_command_enabled);
    }

    #[test]
    fn empty_overrides_are_identity() {
        let prefs = apply_pref_overrides_to(Preferences::default(), &[])
            .expect("empty override list must pass through");
        assert!(!prefs.dom_exec_command_enabled);
    }
}
