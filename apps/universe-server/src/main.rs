use std::path::PathBuf;
use universe_supervisor::Supervisor;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let store = args.next().map(PathBuf::from);
    let genesis = args.next().map(PathBuf::from);
    match (store, genesis) {
        (Some(store), Some(genesis)) => match Supervisor::boot(store, genesis) {
            Ok(supervisor) => println!(
                "ready universe_revision={} tick={}",
                supervisor.revision().0,
                supervisor.tick().0
            ),
            Err(error) => {
                eprintln!("blocked: {error}");
                std::process::exit(1);
            }
        },
        _ => {
            eprintln!("usage: universe-server <store-directory> <genesis-json>");
            std::process::exit(2);
        }
    }
}
