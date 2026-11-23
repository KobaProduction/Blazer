#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(all(debug_assertions, feature = "demo", any(windows, target_os = "linux")))]
#[path = "native/demo_launch.rs"]
mod demo_launch;

#[cfg(windows)]
fn main() -> Result<std::process::ExitCode, Box<dyn std::error::Error>> {
    use std::process::ExitCode;
    use vpn_presentation::localization::{Key, Locale, text};

    let title = text(Locale::Russian, Key::AppName);
    let mut arguments = std::env::args_os().skip(1);
    match arguments.next() {
        None => {
            recover_proxy_for_window()?;
            blazer::run()?;
        }
        Some(argument) if argument == "--system-proxy-guard" && arguments.len() == 0 => {
            return Ok(if client_core::run_system_proxy_guard().is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Some(argument) if argument == "--version" && arguments.len() == 0 => {
            attach_parent_console();
            println!("{title} {}", vpn_application::APP_VERSION);
        }
        Some(argument) if argument == "--cli" => {
            attach_parent_console();
            if let Err(error) = client_core::recover_system_proxy() {
                println!("error={error:?}");
                return Ok(ExitCode::FAILURE);
            }
            return Ok(client_cli::launch(arguments));
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
            recover_proxy_for_window()?;
            blazer::run_import(import.into_owned())?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(windows)]
fn recover_proxy_for_window() -> Result<(), Box<dyn std::error::Error>> {
    if let Err(error) = client_core::recover_system_proxy() {
        use vpn_presentation::localization::{Key, Locale, text};
        rfd::MessageDialog::new()
            .set_title(text(Locale::Russian, Key::AppName))
            .set_description(text(Locale::Russian, Key::SystemProxyRestoreFailed))
            .set_level(rfd::MessageLevel::Error)
            .show();
        return Err(format!("Windows proxy recovery failed: {error:?}").into());
    }
    Ok(())
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
