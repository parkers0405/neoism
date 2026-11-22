use super::*;

/// Older daemon schemas expose only the fixed-width face. Keep both controls available in Rune.
pub(super) fn descriptors(mut descriptors: Vec<ConfigDescriptor>) -> Vec<ConfigDescriptor> {
    let Some(mono) = descriptors.iter_mut().find(|d| d.path == "appearance.fonts.family") else { return descriptors };
    mono.label = "Monospace font family".into();
    mono.description = "Font for the editor, terminal, code blocks, tables and fixed-width interface text.".into();
    mono.default = json!(crate::ui::fonts::MONO);
    mono.static_suggestions.extend(["Geist Mono", "Cascadia Code NF"].map(str::to_owned));
    let mut chat = mono.clone();
    chat.path = "appearance.chat-font-family".into();
    chat.label = "Chat font family".into();
    chat.description = "Font for agent messages, reasoning and the composer. Code keeps the separate monospace font.".into();
    chat.default = json!(crate::ui::fonts::CHAT);
    chat.static_suggestions.extend([crate::ui::fonts::CHAT, "Geist", "Arial", "Helvetica Neue"].map(str::to_owned));
    if let Some(existing) = descriptors.iter_mut().find(|d| d.path == chat.path) {
        existing.static_suggestions.extend(chat.static_suggestions);
        // An older daemon still names the bundled face as the default.
        existing.default = chat.default;
        existing.label = chat.label;
        existing.description = chat.description;
    } else {
        descriptors.push(chat);
    }
    descriptors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_controls_apply_independently_and_follow_config_reload() {
        let mut app = App::new();
        app.open_settings();
        app.apply_config_reply(0, &json!({"Config": {"value": {}}}));
        // The installed daemon may still advertise only the original font field.
        app.apply_config_reply(0, &json!({"ConfigSchema": {"descriptors": [{
            "path": "appearance.fonts.family", "label": "Font family", "description": "",
            "value_kind": "string", "default": "Cascadia Code", "extensible": true,
            "category": "appearance", "control": ConfigControl::FontFamily, "settings_visible": true,
            "runtime_suggestions": ["Menlo", "Helvetica Neue"]
        }]}}));
        for (path, name) in [("appearance.chat-font-family", "Helvetica Neue"), ("appearance.fonts.family", "Menlo")] {
            let settings = app.settings.as_ref().unwrap();
            let index = settings.rows.iter().position(|r| r.path == path).unwrap();
            let options = settings.options(&settings.rows[index]);
            let option = options.iter().position(|(_, value)| value.as_str() == Some(name)).unwrap();
            app.choose_option(index, option);
            assert_eq!(app.settings.as_ref().unwrap().value(path), Some(&json!(name)));
            if path == "appearance.chat-font-family" { assert_eq!(app.font_families, (name.into(), crate::ui::fonts::MONO.into())); }
        }
        let mut rt = rune::Runtime::new(app, |_, _: &mut rune::Cx<App>| rune::el(), (300.0, 100.0), rune::Env::new(true, 1.0));
        rt.update(|app, tx| { app.sync_font_families(tx); });
        assert_eq!(rt.font_families(), ("Helvetica Neue, Geist", "Menlo, Geist Mono"));
        rt.update(|app, tx| {
            app.settings = None;
            app.apply_config_reply(0, &json!({"Config": {"value": {"appearance": {
                "chat-font-family": "Arial", "fonts": {"family": "Geist Mono"}
            }}}}));
            app.sync_font_families(tx);
        });
        assert_eq!(rt.font_families(), ("Arial, Geist", "Geist Mono"));
        rt.update(|app, tx| {
            app.apply_config_reply(0, &json!({"Config": {"value": {}}}));
            app.sync_font_families(tx);
        });
        assert_eq!(rt.font_families(), (crate::ui::fonts::chat_stack(crate::ui::fonts::CHAT).as_str(), crate::ui::fonts::MONO));
    }
}