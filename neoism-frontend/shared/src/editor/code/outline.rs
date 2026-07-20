//! Symbol ancestor trail for the code pane's breadcrumbs: the chain of
//! named containers around the cursor (`mod net → impl Client → fn
//! connect`), extracted by walking the tree-sitter parse tree from the
//! root down to the cursor byte. No `.scm` outline queries — a small
//! per-language node-kind table keeps it dependency-free.

use crate::syntax::Lang;

/// Recompute guard: don't parse huge buffers on every cursor-line move
/// (matches the whole-buffer highlight cutoff).
pub const OUTLINE_SOURCE_CUTOFF: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Impl,
    Class,
    Interface,
    Module,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineSymbol {
    pub name: String,
    pub kind: OutlineKind,
}

/// Containers around `cursor_byte`, outermost first. Empty when the
/// language has no parser, the parse fails, or nothing encloses the
/// cursor.
#[cfg(not(target_arch = "wasm32"))]
pub fn symbol_trail(source: &str, lang: Lang, cursor_byte: usize) -> Vec<OutlineSymbol> {
    let Some(language) = language_for(lang) else {
        return Vec::new();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let byte = cursor_byte.min(source.len());
    let mut out = Vec::new();
    let mut node = tree.root_node();
    loop {
        if let Some(symbol) = symbol_for_node(node, source) {
            out.push(symbol);
            if out.len() >= 8 {
                break;
            }
        }
        let mut next = None;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.start_byte() <= byte && byte <= child.end_byte() {
                next = Some(child);
                break;
            }
        }
        drop(cursor);
        match next {
            Some(child) => node = child,
            None => break,
        }
    }
    out
}

#[cfg(target_arch = "wasm32")]
pub fn symbol_trail(
    _source: &str,
    _lang: Lang,
    _cursor_byte: usize,
) -> Vec<OutlineSymbol> {
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn language_for(lang: Lang) -> Option<tree_sitter::Language> {
    Some(match lang {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::Javascript | Lang::Jsx => tree_sitter_javascript::LANGUAGE.into(),
        Lang::Typescript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::Lua => tree_sitter_lua::LANGUAGE.into(),
        Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
        Lang::C => tree_sitter_c::LANGUAGE.into(),
        Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        // Nix / YAML / CSS / HTML have no named-container node kinds
        // that map onto a useful symbol trail; skip them.
        _ => return None,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn symbol_for_node(node: tree_sitter::Node, source: &str) -> Option<OutlineSymbol> {
    use OutlineKind::*;
    let kind = match node.kind() {
        // rust / go / python / lua / js / bash / c / c++ share most of
        // these kinds (`function_definition` covers python, bash, and
        // C/C++ functions alike).
        "function_item"
        | "function_declaration"
        | "function_definition"
        | "local_function_declaration_statement"
        | "generator_function_declaration" => Function,
        "method_definition" | "method_declaration" => Method,
        "struct_item" | "type_declaration" | "struct_specifier" => Struct,
        "enum_item" | "enum_declaration" => Enum,
        "trait_item" => Trait,
        "impl_item" => Impl,
        "class_declaration" | "class_definition" | "class_specifier" => Class,
        "interface_declaration" => Interface,
        "mod_item" | "module" | "internal_module" | "namespace_declaration" => Module,
        _ => return None,
    };
    let name_node = node
        .child_by_field_name("name")
        // C/C++ `function_definition` buries the identifier inside a
        // (possibly nested) `declarator` chain: function_declarator →
        // pointer_declarator → … → identifier. Walk it best-effort.
        // Checked BEFORE the `type` arm: C's `function_definition` also
        // carries a `type` field (the return type), which must not win.
        .or_else(|| {
            let mut current = node.child_by_field_name("declarator")?;
            for _ in 0..8 {
                match current.child_by_field_name("declarator") {
                    Some(inner) => current = inner,
                    None => break,
                }
            }
            Some(current)
        })
        // Rust `impl_item` labels the implemented type as `type`.
        .or_else(|| node.child_by_field_name("type"))
        // Go wraps the named `type_spec` inside `type_declaration`.
        .or_else(|| {
            let mut cursor = node.walk();
            let spec = node
                .named_children(&mut cursor)
                .find(|child| child.kind() == "type_spec");
            spec.and_then(|spec| spec.child_by_field_name("name"))
        })?;
    let name = name_node
        .utf8_text(source.as_bytes())
        .ok()?
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }
    let mut name = name;
    if name.len() > 64 {
        let cut = (0..=64)
            .rev()
            .find(|ix| name.is_char_boundary(*ix))
            .unwrap_or(0);
        name.truncate(cut);
        name.push('…');
    }
    Some(OutlineSymbol { name, kind })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn rust_trail_walks_mod_impl_fn() {
        let source = "mod net {\n    struct Client;\n    impl Client {\n        fn connect(&self) {\n            let x = 1;\n        }\n    }\n}\n";
        let byte = source.find("let x").unwrap();
        let trail = symbol_trail(source, Lang::Rust, byte);
        let names: Vec<&str> = trail.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["net", "Client", "connect"]);
        assert_eq!(trail[1].kind, OutlineKind::Impl);
    }

    #[test]
    fn python_trail_class_method() {
        let source = "class Repo:\n    def fetch(self):\n        pass\n";
        let byte = source.find("pass").unwrap();
        let trail = symbol_trail(source, Lang::Python, byte);
        let names: Vec<&str> = trail.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Repo", "fetch"]);
    }

    #[test]
    fn c_trail_names_function_through_declarator_chain() {
        let source = "struct point { int x; };\nstatic char *render(struct point *p) {\n    return 0;\n}\n";
        let byte = source.find("return").unwrap();
        let trail = symbol_trail(source, Lang::C, byte);
        let names: Vec<&str> = trail.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["render"]);
        assert_eq!(trail[0].kind, OutlineKind::Function);
    }

    #[test]
    fn cpp_trail_walks_class_method() {
        let source = "class Repo {\n    void fetch() {\n        int x = 1;\n    }\n};\n";
        let byte = source.find("int x").unwrap();
        let trail = symbol_trail(source, Lang::Cpp, byte);
        let names: Vec<&str> = trail.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Repo", "fetch"]);
        assert_eq!(trail[0].kind, OutlineKind::Class);
    }

    #[test]
    fn bash_trail_finds_function() {
        let source = "deploy() {\n    echo hi\n}\n";
        let byte = source.find("echo").unwrap();
        let trail = symbol_trail(source, Lang::Bash, byte);
        let names: Vec<&str> = trail.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["deploy"]);
    }

    #[test]
    fn unsupported_language_is_empty() {
        assert!(symbol_trail("{}", Lang::Json, 0).is_empty());
    }
}
