use vijutsu::subcommands::{
    inspect, mutate, poll, preview, print_bookmarks, print_capabilities, print_heads, print_help,
    print_keys, print_version, snapshot,
};
use vijutsu::tui_loop::run_tui_or_exit;

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("inspect") | None => inspect(args.collect()),
        Some("preview") => preview(args.collect()),
        Some("mutate") => mutate(args.collect()),
        Some("snapshot") => snapshot(args.collect()),
        Some("heads") => print_heads(),
        Some("bookmarks") => print_bookmarks(),
        Some("keys") => print_keys(),
        Some("tui") => run_tui_or_exit(),
        Some("poll") => poll(args.collect()),
        Some("capabilities") => print_capabilities(),
        Some("version") => print_version(),
        Some("help") | Some("--help") | Some("-h") => print_help(),
        Some(other) => {
            eprintln!("unknown command: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}
