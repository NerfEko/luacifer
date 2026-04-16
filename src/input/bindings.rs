use crate::input::Action;

#[cfg(feature = "lua")]
use crate::lua::BindingConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModifierSet {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

impl ModifierSet {
    pub fn from_names(names: &[String]) -> Self {
        let mut set = Self::default();
        for name in names {
            match name.as_str() {
                "Ctrl" | "Control" => set.ctrl = true,
                "Alt" => set.alt = true,
                "Shift" => set.shift = true,
                "Super" | "Logo" | "Meta" => set.logo = true,
                _ => {}
            }
        }
        set
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionBinding {
    pub modifiers: ModifierSet,
    pub key: String,
    pub action: Action,
}

impl ActionBinding {
    pub fn matches(&self, key: &str, modifiers: ModifierSet) -> bool {
        self.key.eq_ignore_ascii_case(key) && self.modifiers == modifiers
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BindingMap {
    bindings: Vec<ActionBinding>,
}

impl BindingMap {
    #[cfg(feature = "lua")]
    pub fn from_config(bindings: &[BindingConfig], default_pan: f64, default_zoom: f64) -> Self {
        let bindings = bindings
            .iter()
            .filter_map(|binding| {
                Action::from_name(
                    &binding.action,
                    binding.amount,
                    default_pan,
                    default_zoom,
                    binding.command.as_deref(),
                )
                .map(|action| ActionBinding {
                    modifiers: ModifierSet::from_names(&binding.mods),
                    key: normalize_key(&binding.key),
                    action,
                })
            })
            .collect();

        Self { bindings }
    }

    pub fn resolve(&self, key: &str, modifiers: ModifierSet) -> Option<Action> {
        let key = normalize_key(key);
        self.bindings
            .iter()
            .find(|binding| binding.matches(&key, modifiers))
            .map(|binding| binding.action.clone())
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

pub fn parse_keyspec(spec: &str) -> Result<(Vec<String>, String), String> {
    let parts = spec
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    let Some((key, modifiers)) = parts.split_last() else {
        return Err("binding keyspec must not be empty".into());
    };

    let mut mods = Vec::new();
    for modifier in modifiers {
        let canonical = canonical_modifier_name(modifier)
            .ok_or_else(|| format!("unsupported modifier in keyspec: {modifier}"))?;
        mods.push(canonical.to_string());
    }

    Ok((mods, normalize_key(key)))
}

pub fn normalize_key(key: &str) -> String {
    let key = key.trim();
    let lower = key.to_ascii_lowercase();
    match lower.as_str() {
        "-" | "minus" => "Minus".to_string(),
        "=" | "equal" => "Equal".to_string(),
        "space" => "Space".to_string(),
        "return" | "enter" => "Return".to_string(),
        "escape" | "esc" => "Escape".to_string(),
        "tab" => "Tab".to_string(),
        "backspace" => "Backspace".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "home" => "Home".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        _ if key.chars().count() == 1 => key.to_ascii_uppercase(),
        _ => lower,
    }
}

pub(crate) fn canonical_modifier_name(name: &str) -> Option<&'static str> {
    match name {
        "Ctrl" | "Control" => Some("Ctrl"),
        "Alt" => Some("Alt"),
        "Shift" => Some("Shift"),
        "Super" | "Logo" | "Meta" => Some("Super"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_key, parse_keyspec};

    #[test]
    fn parses_super_h_keyspec() {
        let (mods, key) = parse_keyspec("Super+H").expect("parse keyspec");
        assert_eq!(mods, vec!["Super"]);
        assert_eq!(key, "H");
    }

    #[test]
    fn parses_bare_keyspec() {
        let (mods, key) = parse_keyspec("Equal").expect("parse keyspec");
        assert!(mods.is_empty());
        assert_eq!(key, "Equal");
    }

    #[test]
    fn rejects_unknown_modifier() {
        let error = parse_keyspec("Hyper+H").expect_err("unknown modifier should fail");
        assert!(error.contains("unsupported modifier"));
    }

    #[test]
    fn normalizes_symbolic_keys_case_insensitively() {
        assert_eq!(normalize_key("minus"), "Minus");
        assert_eq!(normalize_key("Minus"), "Minus");
        assert_eq!(normalize_key("equal"), "Equal");
        assert_eq!(normalize_key("SPACE"), "Space");
        assert_eq!(normalize_key("Enter"), "Return");
    }

    #[test]
    fn normalizes_unknown_multi_char_keys_consistently() {
        assert_eq!(normalize_key("XF86AudioRaiseVolume"), "xf86audioraisevolume");
        assert_eq!(normalize_key("xf86audioraisevolume"), "xf86audioraisevolume");
        assert_eq!(normalize_key("  Print  "), "print");
    }
}
