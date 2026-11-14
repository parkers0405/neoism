//! Rune's role names (ARIA's, plus a few Rune components use) as AccessKit roles.

use accesskit::Role;

/// What an element's role string says, given what else the element is.
pub(crate) struct RoleCtx {
    pub pressable: bool,
    pub input: bool,
    pub multiline: bool,
    pub secure: bool,
    pub scrolls: bool,
    pub modal: bool,
    pub default_button: bool,
    pub labelled: bool,
    pub keyed: bool,
    pub has_text: bool,
    /// It shows a picture or video (`El::pixels`, `El::video`).
    pub image: bool,
}

/// The AccessKit role for `role`, or `None` when the element adds nothing to the tree and its
/// children belong to its parent (`"none"`, `"presentation"`, an unlabelled decorative icon,
/// a plain layout box).
pub(crate) fn map(role: &str, cx: &RoleCtx) -> Option<Role> {
    if cx.input {
        return Some(if cx.secure { Role::PasswordInput }
            else if role == "searchbox" { Role::SearchInput }
            else if cx.multiline { Role::MultilineTextInput } else { Role::TextInput });
    }
    let known = match role {
        "button" | "toggle" | "swatch" | "sidebar-toggle" => {
            if cx.default_button { Role::DefaultButton } else { Role::Button }
        }
        "checkbox" => Role::CheckBox,
        "switch" => Role::Switch,
        "radio" | "segment" => Role::RadioButton,
        "radiogroup" | "segmented" => Role::RadioGroup,
        "slider" => Role::Slider,
        "spinbutton" => Role::SpinButton,
        "link" => Role::Link,
        "heading" | "section-header" => Role::Heading,
        "text" | "label" | "mark" => Role::Label,
        "paragraph" => Role::Paragraph,
        "image" | "img" => Role::Image,
        "icon" => if cx.labelled || cx.keyed { Role::Image } else { return None },
        "canvas" => Role::Canvas,
        "list" | "sidebar-list" => Role::List,
        "listitem" => Role::ListItem,
        "listbox" => Role::ListBox,
        "option" => Role::ListBoxOption,
        "tree" => Role::Tree,
        "treeitem" => Role::TreeItem,
        "treegrid" => Role::TreeGrid,
        "grid" => Role::Grid,
        "table" => Role::Table,
        "row" => Role::Row,
        "rowgroup" => Role::RowGroup,
        "gridcell" => Role::GridCell,
        "cell" => Role::Cell,
        "columnheader" => Role::ColumnHeader,
        "rowheader" => Role::RowHeader,
        "menu" => Role::Menu,
        "menubar" => Role::MenuBar,
        "menuitem" => Role::MenuItem,
        "menuitemcheckbox" => Role::MenuItemCheckBox,
        "menuitemradio" => Role::MenuItemRadio,
        "tab" => Role::Tab,
        "tablist" => Role::TabList,
        "tabpanel" => Role::TabPanel,
        "dialog" => Role::Dialog,
        "alertdialog" => Role::AlertDialog,
        "alert" => Role::Alert,
        "status" => Role::Status,
        "log" => Role::Log,
        "timer" => Role::Timer,
        "tooltip" => Role::Tooltip,
        "progressbar" => Role::ProgressIndicator,
        "meter" => Role::Meter,
        // A rule between sections is decoration; one that drags resizes them.
        "separator" | "sidebar-grip" => if cx.pressable { Role::Splitter } else { return None },
        "toolbar" => Role::Toolbar,
        "region" => Role::Region,
        "combobox" => Role::ComboBox,
        "window" => Role::Window,
        "note" => Role::Note,
        "document" => Role::Document,
        "article" => Role::Article,
        "navigation" => Role::Navigation,
        "main" => Role::Main,
        "banner" => Role::Banner,
        "complementary" => Role::Complementary,
        "contentinfo" => Role::ContentInfo,
        "form" => Role::Form,
        "search" => Role::Search,
        "figure" => Role::Figure,
        "code" => Role::Code,
        "blockquote" => Role::Blockquote,
        // A box around a field that only routes its presses to the editor: the editor is the
        // text box. Kept as a group when keyed, so drivers still find it by key.
        "textbox" | "searchbox" => if cx.keyed { Role::Group } else { return None },
        "none" | "presentation" => return None,
        "group" => return group(cx),
        _ => return Some(if cx.pressable { Role::Button } else { group(cx).unwrap_or(Role::Group) }),
    };
    Some(known)
}

/// A plain box: a dialog when modal, a scroll view when it scrolls, a button when pressable,
/// a label when it holds text; kept as a group when an app keyed or labelled it, so drivers
/// can find it by key; otherwise left out, its children joining its parent.
fn group(cx: &RoleCtx) -> Option<Role> {
    if cx.modal { return Some(Role::Dialog); }
    if cx.pressable { return Some(if cx.default_button { Role::DefaultButton } else { Role::Button }); }
    if cx.scrolls { return Some(Role::ScrollView); }
    if cx.has_text { return Some(Role::Label); }
    if cx.image && (cx.keyed || cx.labelled) { return Some(Role::Image); }
    (cx.keyed || cx.labelled).then_some(Role::Group)
}

/// Roles whose name AccessKit doesn't take from their descendants' text, but a reader expects
/// to: Rune names them from the text inside when the app gave no label.
pub(crate) fn named_from_content(role: Role) -> bool {
    // Containers (groups, dialogs, toolbars) are named by their label alone: their content is
    // what a reader moves through.
    matches!(role, Role::ListItem | Role::ListBoxOption | Role::TreeItem | Role::Tab | Role::Switch | Role::Row
        | Role::Cell | Role::GridCell | Role::ColumnHeader | Role::RowHeader | Role::Tooltip | Role::Status | Role::Heading
        | Role::Alert | Role::MenuItemCheckBox | Role::MenuItemRadio | Role::DefaultButton)
}

/// Roles whose items carry a selected state (UIA SelectionItem, AX selected).
pub(crate) fn selectable_item(role: Role) -> bool {
    matches!(role, Role::ListBoxOption | Role::ListItem | Role::Tab | Role::TreeItem | Role::GridCell | Role::Row | Role::MenuListOption)
}

/// Roles that take their name from the text inside them through AccessKit itself.
pub(crate) fn named_by_accesskit(role: Role) -> bool {
    matches!(role, Role::Button | Role::CheckBox | Role::Link | Role::MenuItem | Role::RadioButton)
}

/// A stable lower-case name for `role`, for `--tree` and goldens.
pub(crate) fn name(role: Role) -> String {
    let debug = format!("{role:?}");
    let mut out = String::with_capacity(debug.len() + 4);
    for (i, c) in debug.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 { out.push('-'); }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> RoleCtx {
        RoleCtx { pressable: false, input: false, multiline: false, secure: false, scrolls: false, modal: false,
            default_button: false, labelled: false, keyed: false, has_text: false, image: false }
    }

    #[test]
    fn maps_aria_roles_and_rune_component_roles() {
        assert_eq!(map("button", &cx()), Some(Role::Button));
        assert_eq!(map("button", &RoleCtx { default_button: true, ..cx() }), Some(Role::DefaultButton));
        assert_eq!(map("switch", &cx()), Some(Role::Switch));
        assert_eq!(map("option", &cx()), Some(Role::ListBoxOption));
        assert_eq!(map("progressbar", &cx()), Some(Role::ProgressIndicator));
        assert_eq!(map("segmented", &cx()), Some(Role::RadioGroup));
        assert_eq!(map("none", &RoleCtx { pressable: true, ..cx() }), None);
    }

    #[test]
    fn text_fields_take_their_kind_from_the_editor() {
        let field = RoleCtx { input: true, ..cx() };
        assert_eq!(map("textbox", &field), Some(Role::TextInput));
        assert_eq!(map("textbox", &RoleCtx { multiline: true, ..field }), Some(Role::MultilineTextInput));
        assert_eq!(map("textbox", &RoleCtx { secure: true, ..field }), Some(Role::PasswordInput));
    }

    #[test]
    fn plain_boxes_join_their_parent_unless_they_mean_something() {
        assert_eq!(map("group", &cx()), None);
        assert_eq!(map("group", &RoleCtx { keyed: true, ..cx() }), Some(Role::Group));
        assert_eq!(map("group", &RoleCtx { scrolls: true, ..cx() }), Some(Role::ScrollView));
        assert_eq!(map("group", &RoleCtx { pressable: true, ..cx() }), Some(Role::Button));
        assert_eq!(map("group", &RoleCtx { modal: true, ..cx() }), Some(Role::Dialog));
        assert_eq!(map("icon", &cx()), None);
        assert_eq!(map("icon", &RoleCtx { labelled: true, ..cx() }), Some(Role::Image));
        assert_eq!(map("whatever", &RoleCtx { pressable: true, ..cx() }), Some(Role::Button));
    }

    #[test]
    fn names_are_kebab_case() {
        assert_eq!(name(Role::ListBoxOption), "list-box-option");
        assert_eq!(name(Role::Button), "button");
    }
}
