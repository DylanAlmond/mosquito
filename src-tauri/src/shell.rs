use std::io;

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

pub fn register_context_menu() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let classes = RegKey::predef(HKEY_CURRENT_USER);
    let (verb, _) = classes.create_subkey(r"Software\Classes\*\shell\Mosquito.Share")?;

    verb.set_value("MUIVerb", &"Share with Mosquito")?;
    verb.set_value("MultiSelectModel", &"Document")?;
    verb.set_value("Icon", &exe.to_string_lossy().to_string())?;

    let (command, _) = verb.create_subkey("command")?;
    let command_line = format!("\"{}\" --share \"%1\"", exe.display());
    command.set_value("", &command_line)
}
