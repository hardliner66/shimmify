use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Default, Subcommand)]
enum Action {
    #[default]
    Run,
    Shim(shimmify::ShimmifyArgs),
}

fn main() -> anyhow::Result<()> {
    let Cli { action } = Cli::parse();
    match action.unwrap_or_default() {
        Action::Shim(shimmify) => shimmify.exec("shimmify.toml", None)?,
        Action::Run => (),
    }
    println!("Unshimmed!");
    Ok(())
}
