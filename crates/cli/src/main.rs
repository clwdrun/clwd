use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "clwd", version, about = "Deploy and share small software")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Deploy an app
    Deploy,
    /// Stream logs for an app
    Logs,
    /// Manage sharing for an app
    Share,
    /// Manage secrets for an app
    Secrets,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Deploy => println!("TODO: deploy"),
        Commands::Logs => println!("TODO: logs"),
        Commands::Share => println!("TODO: share"),
        Commands::Secrets => println!("TODO: secrets"),
    }
    Ok(())
}
