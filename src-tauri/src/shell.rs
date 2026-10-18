//! Windows Explorer registration for the "Share with Mosquito" context
//! menu.
//!
//! The values the app publishes to the registry live in the
//! [`registration`] submodule, compiled wherever tests can reach them so
//! debug builds can pin them (issue #18): the tests below pin the
//! currently-published values — legacy `*`-class key, `Document`
//! multi-select model — so ticket #19, which changes the registered key,
//! can flip the pins and prove what changed.
//!
//! The only release-gated part is the thin `register_context_menu` glue,
//! which writes those values with `winreg`. It stays
//! `not(debug_assertions)` the way the module always has: a debug build
//! must not write anything to the registry.

#[cfg(all(windows, not(debug_assertions)))]
use std::io;

#[cfg(any(test, all(windows, not(debug_assertions))))]
pub mod registration {
    use std::path::Path;

    /// Registry path of the verb key, relative to `HKEY_CURRENT_USER`.
    ///
    /// Still the legacy `*` (any file type) class key. Pinned by the
    /// tests below; #19 moves the verb to a narrower class key and flips
    /// this pin.
    pub const VERB_KEY_PATH: &str = r"Software\Classes\*\shell\Mosquito.Share";

    /// Name of the subkey of the verb key that holds the invocation.
    pub const COMMAND_SUBKEY: &str = "command";

    /// The menu entry text, written to the verb key's `MUIVerb` value.
    pub const VERB_NAME: &str = "Share with Mosquito";

    /// How the menu entry reports multi-file selection to Explorer.
    pub const MULTI_SELECT_MODEL: &str = "Document";

    /// The command subkey's default value: launch this app passing the
    /// first selected file to `--share`.
    pub fn command_line(exe_path: &Path) -> String {
        format!("\"{}\" --share \"%1\"", exe_path.display())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn verb_key_path_pins_the_legacy_star_class_key() {
            assert_eq!(VERB_KEY_PATH, r"Software\Classes\*\shell\Mosquito.Share");
        }

        #[test]
        fn command_subkey_pins_the_standard_command_name() {
            assert_eq!(COMMAND_SUBKEY, "command");
        }

        #[test]
        fn verb_name_pins_the_menu_entry_text() {
            assert_eq!(VERB_NAME, "Share with Mosquito");
        }

        #[test]
        fn multi_select_model_pins_the_document_model() {
            assert_eq!(MULTI_SELECT_MODEL, "Document");
        }

        #[test]
        fn command_line_pins_the_share_invocation() {
            let command = command_line(Path::new(r"C:\Apps\Mosquito.exe"));

            assert_eq!(command, r#""C:\Apps\Mosquito.exe" --share "%1""#);
        }
    }
}

// Thin glue: release-Windows-only, no decisions of its own. Everything
// you can pin is above; this only performs the mechanical writes.
#[cfg(all(windows, not(debug_assertions)))]
pub fn register_context_menu() -> io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let exe = std::env::current_exe()?;
    let classes = RegKey::predef(HKEY_CURRENT_USER);
    let (verb, _) = classes.create_subkey(registration::VERB_KEY_PATH)?;

    verb.set_value("MUIVerb", &registration::VERB_NAME)?;
    verb.set_value("MultiSelectModel", &registration::MULTI_SELECT_MODEL)?;
    verb.set_value("Icon", &exe.to_string_lossy().to_string())?;

    let (command, _) = verb.create_subkey(registration::COMMAND_SUBKEY)?;
    command.set_value("", &registration::command_line(&exe))
}
