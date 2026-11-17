#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(all(debug_assertions, feature = "demo", any(windows, target_os = "linux")))]
#[path = "native/demo_launch.rs"]
mod demo_launch;

#[cfg(windows)]
fn main() -> Result<std::process::ExitCode, Box<dyn std::error::Error>> {
    use std::process::ExitCode;

    let mut arguments = std::env::args_os().skip(1);
    match arguments.next() {
        None => blazer::run()?,
        Some(argument) if argument == "--version" && arguments.len() == 0 => {
            attach_parent_console();
            println!("Blazer {}", vpn_application::APP_VERSION);
        }
        Some(argument) if argument == "--cli" => {
            attach_parent_console();
            return Ok(client_cli::launch(arguments));
        }
        Some(argument) if argument == "--apply-update" => {
            if let Err(error) = blazer::apply_update(arguments) {
                rfd::MessageDialog::new()
                    .set_title("Blazer")
                    .set_description(format!("Не удалось установить обновление.\n{error}"))
                    .set_level(rfd::MessageLevel::Error)
                    .show();
                return Ok(ExitCode::FAILURE);
            }
        }
        Some(argument) if argument == "--finish-update" && arguments.len() == 1 => {
            blazer::run_after_update(arguments.next().expect("one update directory argument"))?;
        }
        #[cfg(all(debug_assertions, feature = "demo"))]
        Some(argument) if argument == "--demo" => demo_launch::run(arguments)?,
        Some(argument) => {
            let import = argument
                .to_str()
                .and_then(vpn_presentation::import::import_link);
            let Some(import) = import.filter(|_| arguments.next().is_none()) else {
                attach_parent_console();
                return Err("usage: blazer [--version | --cli | blazer://add/{url}]".into());
            };
            blazer::run_import(import.into_owned())?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(windows)]
fn attach_parent_console() {
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    let _ = win32console::console::WinConsole::attach_console(ATTACH_PARENT_PROCESS);
}

#[cfg(all(debug_assertions, feature = "demo", target_os = "linux"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments
        .next()
        .is_some_and(|argument| argument == "--demo")
    {
        return demo_launch::run(arguments);
    }
    Err("Build blazer-linux to launch the Linux client".into())
}

#[cfg(not(any(windows, all(debug_assertions, feature = "demo", target_os = "linux"))))]
fn main() -> std::process::ExitCode {
    eprintln!("Build the Blazer package for your operating system.");
    std::process::ExitCode::FAILURE
}
