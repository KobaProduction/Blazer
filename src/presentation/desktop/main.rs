#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let Some(argument) = arguments.next() else {
        return Ok(vpn_client::run()?);
    };
    let import = argument
        .to_str()
        .and_then(vpn_presentation::import::import_link);
    let Some(import) = import.filter(|_| arguments.next().is_none()) else {
        return Err("usage: vpn-client [blazer://add/{url}]".into());
    };
    Ok(vpn_client::run_import(import.into_owned())?)
}
