use std::process::Command;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

fn main() {
    let vijutsu = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("vijutsu")))
        .unwrap_or_else(|| "vijutsu".into());

    #[cfg(unix)]
    {
        let error = Command::new(vijutsu).arg("tui").exec();
        eprintln!("failed to launch vijutsu tui: {error}");
    }

    #[cfg(not(unix))]
    {
        let status = Command::new(vijutsu).arg("tui").status();
        match status {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(error) => {
                eprintln!("failed to launch vijutsu tui: {error}");
                std::process::exit(1);
            }
        }
    }
}
