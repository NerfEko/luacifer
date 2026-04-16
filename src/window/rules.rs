use crate::{canvas::Size, window::model::WindowProperties};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WindowRule {
    pub app_id: Option<String>,
    pub title_contains: Option<String>,
    pub floating: Option<bool>,
    pub exclude_from_focus: Option<bool>,
    pub default_size: Option<Size>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppliedWindowRules {
    pub floating: Option<bool>,
    pub exclude_from_focus: Option<bool>,
    pub default_size: Option<Size>,
}

impl WindowRule {
    pub fn matches(&self, properties: &WindowProperties) -> bool {
        let app_id_matches = self.app_id.as_ref().is_none_or(|expected| {
            properties
                .app_id
                .as_ref()
                .is_some_and(|actual| actual == expected)
        });
        let title_matches = self.title_contains.as_ref().is_none_or(|needle| {
            properties
                .title
                .as_ref()
                .is_some_and(|title| title.contains(needle))
        });

        app_id_matches && title_matches
    }
}

impl AppliedWindowRules {
    pub fn from_rules(properties: &WindowProperties, rules: &[WindowRule]) -> Self {
        let mut applied = Self::default();

        for rule in rules.iter().filter(|rule| rule.matches(properties)) {
            if let Some(floating) = rule.floating {
                applied.floating = Some(floating);
            }
            if let Some(exclude_from_focus) = rule.exclude_from_focus {
                applied.exclude_from_focus = Some(exclude_from_focus);
            }
            if let Some(default_size) = rule.default_size {
                applied.default_size = Some(default_size);
            }
        }

        applied
    }
}
