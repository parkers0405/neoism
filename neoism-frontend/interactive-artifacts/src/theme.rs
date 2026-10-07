//! Small, renderer-neutral theme snapshots for trusted experimental artifacts.
//! Values are data, never author-provided CSS declarations or a privileged bridge.
use std::collections::BTreeMap;

/// RGB colors are 24-bit integers, not packed RGBA or CSS expressions.
#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "ipc", serde(deny_unknown_fields))]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArtifactColors {
    pub background: u32,
    pub foreground: u32,
    pub card: u32,
    pub card_foreground: u32,
    pub popover: u32,
    pub popover_foreground: u32,
    pub muted: u32,
    pub muted_foreground: u32,
    pub border: u32,
    pub input: u32,
    pub primary: u32,
    pub primary_foreground: u32,
    pub accent: u32,
    pub accent_foreground: u32,
    pub destructive: u32,
    pub destructive_foreground: u32,
    pub success: u32,
    pub warning: u32,
    pub info: u32,
    pub chart_1: u32,
    pub chart_2: u32,
    pub chart_3: u32,
    pub chart_4: u32,
    pub chart_5: u32,
    pub chart_6: u32,
}

/// A bounded, fixed set of tokens; there are no arbitrary variable names or CSS
/// fragments. Font fields are plain single-family names. Radius is CSS pixels.
#[cfg_attr(feature = "ipc", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "ipc", serde(deny_unknown_fields))]
#[derive(Clone, Debug, PartialEq)]
pub struct ArtifactStyles {
    pub colors: ArtifactColors,
    pub font_sans: String,
    pub font_mono: String,
    pub radius: f32,
}

/// Neutral construction default for tests/clients without a renderer. Desktop
/// always supplies actual selected theme tokens, not this all-zero color set.
impl Default for ArtifactStyles {
    fn default() -> Self {
        Self {
            colors: ArtifactColors::default(),
            font_sans: "sans-serif".into(),
            font_mono: "monospace".into(),
            radius: 8.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StyleError {
    InvalidColor(&'static str),
    InvalidFont(&'static str),
    InvalidRadius,
}
impl std::fmt::Display for StyleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StyleError {}

impl ArtifactColors {
    fn tokens(&self) -> [(&'static str, u32); 25] {
        [
            ("--background", self.background),
            ("--foreground", self.foreground),
            ("--card", self.card),
            ("--card-foreground", self.card_foreground),
            ("--popover", self.popover),
            ("--popover-foreground", self.popover_foreground),
            ("--muted", self.muted),
            ("--muted-foreground", self.muted_foreground),
            ("--border", self.border),
            ("--input", self.input),
            ("--primary", self.primary),
            ("--primary-foreground", self.primary_foreground),
            ("--accent", self.accent),
            ("--accent-foreground", self.accent_foreground),
            ("--destructive", self.destructive),
            ("--destructive-foreground", self.destructive_foreground),
            ("--success", self.success),
            ("--warning", self.warning),
            ("--info", self.info),
            ("--chart-1", self.chart_1),
            ("--chart-2", self.chart_2),
            ("--chart-3", self.chart_3),
            ("--chart-4", self.chart_4),
            ("--chart-5", self.chart_5),
            ("--chart-6", self.chart_6),
        ]
    }
}

impl ArtifactStyles {
    pub fn validate(&self) -> Result<(), StyleError> {
        for (name, color) in self.colors.tokens() {
            if color > 0xFFFFFF {
                return Err(StyleError::InvalidColor(name));
            }
        }
        for (name, family) in [
            ("--font-sans", &self.font_sans),
            ("--font-mono", &self.font_mono),
        ] {
            if family.trim().is_empty()
                || family.len() > 256
                || family.chars().any(char::is_control)
            {
                return Err(StyleError::InvalidFont(name));
            }
        }
        if !self.radius.is_finite() || !(0.0..=64.0).contains(&self.radius) {
            return Err(StyleError::InvalidRadius);
        }
        Ok(())
    }

    /// Validated CSS values for root.style.setProperty or a trusted bootstrap
    /// stylesheet. Colors use full RGB hex (not HSL channel fragments). Quotes,
    /// backslashes, markup delimiters and non-ASCII font characters are escaped
    /// as CSS codepoints, so concatenation cannot introduce a closing style tag.
    /// Escaping is an injection precaution, NOT an engine sandbox.
    pub fn css_variables(&self) -> Result<BTreeMap<String, String>, StyleError> {
        self.validate()?;
        let mut vars: BTreeMap<_, _> = self
            .colors
            .tokens()
            .into_iter()
            .map(|(name, color)| (name.to_owned(), format!("#{color:06x}")))
            .collect();
        vars.insert(
            "--font-sans".into(),
            font_stack(&self.font_sans, "sans-serif"),
        );
        vars.insert(
            "--font-mono".into(),
            font_stack(&self.font_mono, "monospace"),
        );
        vars.insert("--radius".into(), format!("{}px", self.radius));
        Ok(vars)
    }
}

fn font_stack(family: &str, generic: &str) -> String {
    let family = family.trim();
    if family == generic {
        return generic.into();
    }
    let mut escaped = String::with_capacity(family.len());
    for ch in family.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_') {
            escaped.push(ch);
        } else {
            use std::fmt::Write;
            let _ = write!(escaped, "\\{:x} ", ch as u32);
        }
    }
    format!("\"{escaped}\", {generic}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_values_are_full_zero_padded_rgb_and_variable_names_are_fixed() {
        let mut styles = ArtifactStyles::default();
        styles.colors.background = 0x00010f;
        styles.colors.primary = 0xabcdef;
        let vars = styles.css_variables().unwrap();
        assert_eq!(vars["--background"], "#00010f");
        assert_eq!(vars["--primary"], "#abcdef");
        assert_eq!(vars.len(), 28);
        assert!(!vars.keys().any(|name| name.starts_with("--neoism-")));
        styles.colors.background = 0xff000000;
        assert_eq!(
            styles.validate(),
            Err(StyleError::InvalidColor("--background"))
        );
    }

    #[test]
    fn font_names_cannot_close_styles_or_escape_css_strings() {
        let mut styles = ArtifactStyles::default();
        styles.font_sans = "Family \"</style>\\ä".into();
        let vars = styles.css_variables().unwrap();
        assert_eq!(
            vars["--font-sans"],
            "\"Family \\22 \\3c \\2f style\\3e \\5c \\e4 \", sans-serif"
        );
        assert!(!vars["--font-sans"].contains("</style>"));
        assert_eq!(vars["--font-mono"], "monospace");
        styles.font_sans = "bad\nfont".into();
        assert_eq!(
            styles.validate(),
            Err(StyleError::InvalidFont("--font-sans"))
        );
        styles.font_sans = "x".repeat(257);
        assert!(styles.validate().is_err());
    }

    #[test]
    fn radius_is_a_bounded_finite_dimension() {
        for radius in [f32::NAN, f32::INFINITY, -1.0, 65.0] {
            let styles = ArtifactStyles {
                radius,
                ..Default::default()
            };
            assert_eq!(styles.validate(), Err(StyleError::InvalidRadius));
        }
        assert_eq!(
            ArtifactStyles {
                radius: 4.5,
                ..Default::default()
            }
            .css_variables()
            .unwrap()["--radius"],
            "4.5px"
        );
    }

    #[cfg(feature = "ipc")]
    #[test]
    fn ipc_roundtrip_and_unknown_tokens_are_rejected() {
        let styles = ArtifactStyles::default();
        let bytes = serde_json::to_vec(&styles).unwrap();
        assert_eq!(
            serde_json::from_slice::<ArtifactStyles>(&bytes).unwrap(),
            styles
        );
        let mut value = serde_json::to_value(&styles).unwrap();
        value["colors"]["custom_css"] = serde_json::json!("url(file:///etc/passwd)");
        assert!(serde_json::from_value::<ArtifactStyles>(value).is_err());
    }
}
