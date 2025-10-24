use argh::FromArgs;
use embuer::dbus::EmbuerDBusProxy;
use futures_util::StreamExt;
use owo_colors::OwoColorize;
use std::process;
use zbus::Connection;

/// Embuer Client - Control and monitor the Embuer update service
#[derive(FromArgs)]
struct EmbuerCli {
    #[argh(subcommand)]
    command: SubCommand,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCommand {
    Status(StatusCmd),
    BootInfo(BootInfoCmd),
    Watch(WatchCmd),
    InstallFile(InstallFileCmd),
    InstallUrl(InstallUrlCmd),
    PendingUpdate(PendingUpdateCmd),
    Accept(AcceptCmd),
    Reject(RejectCmd),
}

/// Get the current update status
#[derive(FromArgs)]
#[argh(subcommand, name = "status")]
struct StatusCmd {}

/// Get information about the currently running deployment
#[derive(FromArgs)]
#[argh(subcommand, name = "boot-info")]
struct BootInfoCmd {}

/// Watch for status changes in real-time
#[derive(FromArgs)]
#[argh(subcommand, name = "watch")]
struct WatchCmd {}

/// Install an update from a local file
#[derive(FromArgs)]
#[argh(subcommand, name = "install-file")]
struct InstallFileCmd {
    /// path to the update file
    #[argh(positional)]
    path: String,
}

/// Install an update from a URL
#[derive(FromArgs)]
#[argh(subcommand, name = "install-url")]
struct InstallUrlCmd {
    /// URL to download the update from
    #[argh(positional)]
    url: String,
}

/// Show details of pending update awaiting confirmation
#[derive(FromArgs)]
#[argh(subcommand, name = "pending-update")]
struct PendingUpdateCmd {}

/// Accept the pending update and proceed with installation
#[derive(FromArgs)]
#[argh(subcommand, name = "accept")]
struct AcceptCmd {}

/// Reject the pending update
#[derive(FromArgs)]
#[argh(subcommand, name = "reject")]
struct RejectCmd {}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    let cli: EmbuerCli = argh::from_env();

    let result = match cli.command {
        SubCommand::Status(_) => get_status().await,
        SubCommand::BootInfo(_) => get_boot_info().await,
        SubCommand::Watch(_) => watch_status().await,
        SubCommand::InstallFile(cmd) => install_from_file(&cmd.path).await,
        SubCommand::InstallUrl(cmd) => install_from_url(&cmd.url).await,
        SubCommand::PendingUpdate(_) => get_pending_update().await,
        SubCommand::Accept(_) => confirm_update(true).await,
        SubCommand::Reject(_) => confirm_update(false).await,
    };

    if let Err(e) = result {
        eprintln!("{} {}", "✗".red().bold(), e.to_string().red());
        process::exit(1);
    }
}

async fn get_connection() -> Result<Connection, Box<dyn std::error::Error>> {
    let connection = Connection::system().await?;
    Ok(connection)
}

async fn get_status() -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    let (status, details, progress) = proxy.get_update_status().await?;

    println!(
        "{} {}",
        "📊".bright_cyan(),
        "Update Status".bright_white().bold()
    );
    println!();

    let status_colored = match status.as_str() {
        "Idle" => status.dimmed().to_string(),
        "Checking" => status.bright_blue().to_string(),
        "Clearing" => status.bright_yellow().to_string(),
        "Downloading" => status.bright_cyan().to_string(),
        "Installing" => status.bright_magenta().to_string(),
        "AwaitingConfirmation" => status.bright_yellow().bold().to_string(),
        "Completed" => status.bright_green().bold().to_string(),
        "Failed" => status.bright_red().bold().to_string(),
        _ => status.white().to_string(),
    };

    println!("  {} {}", "Status:".bright_white(), status_colored);

    if !details.is_empty() {
        println!("  {} {}", "Details:".bright_white(), details.cyan());
    }

    if progress >= 0 {
        let progress_bar = create_progress_bar(progress);
        println!(
            "  {} {}% {}",
            "Progress:".bright_white(),
            progress.to_string().bright_green(),
            progress_bar
        );
    }

    Ok(())
}

async fn get_boot_info() -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    let (boot_id, boot_name) = proxy.get_boot_info().await?;

    println!(
        "{} {}",
        "🚀".bright_magenta(),
        "Boot Deployment Information".bright_white().bold()
    );
    println!();
    println!(
        "  {} {}",
        "Deployment Name:".bright_white(),
        boot_name.bright_cyan().bold()
    );
    println!(
        "  {} {}",
        "Subvolume ID:".bright_white(),
        boot_id.to_string().bright_green()
    );

    Ok(())
}

async fn watch_status() -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    println!(
        "{} {}",
        "👁️".bright_yellow(),
        "Watching for status updates...".bright_white().bold()
    );
    println!("{}", "(Press Ctrl+C to exit)".dimmed());
    println!();

    // Get initial status
    let (status, details, progress) = proxy.get_update_status().await?;
    print_status_line(&status, &details, progress);

    // Subscribe to status change signals
    let mut stream = proxy.receive_update_status_changed().await?;

    while let Some(signal) = stream.next().await {
        let args = signal.args()?;
        print_status_line(args.status, args.details, args.progress);
    }

    Ok(())
}

fn print_status_line(status: &str, details: &str, progress: i32) {
    let timestamp_str = chrono::Local::now().format("%H:%M:%S").to_string();
    let timestamp = timestamp_str.dimmed();

    let (emoji, status_colored) = match status {
        "Idle" => ("💤", status.dimmed().to_string()),
        "Checking" => ("🔍", status.bright_blue().to_string()),
        "Clearing" => ("🧹", status.bright_yellow().to_string()),
        "Downloading" => ("⬇️", status.bright_cyan().to_string()),
        "Installing" => ("⚙️", status.bright_magenta().to_string()),
        "AwaitingConfirmation" => ("⏸️", status.bright_yellow().bold().to_string()),
        "Completed" => ("✅", status.bright_green().bold().to_string()),
        "Failed" => ("❌", status.bright_red().bold().to_string()),
        _ => ("ℹ️", status.white().to_string()),
    };

    if progress >= 0 {
        let progress_bar = create_progress_bar(progress);
        println!(
            "[{}] {} {} {} {}% {}",
            timestamp,
            emoji,
            status_colored,
            details.cyan(),
            progress.to_string().bright_green(),
            progress_bar
        );
    } else {
        println!(
            "[{}] {} {} {}",
            timestamp,
            emoji,
            status_colored,
            details.cyan()
        );
    }
}

fn create_progress_bar(progress: i32) -> String {
    let filled = (progress / 5) as usize;
    let empty = 20 - filled;
    let bar = format!(
        "{}{}",
        "█".repeat(filled).bright_green(),
        "░".repeat(empty).dimmed()
    );
    format!("[{}]", bar)
}

async fn install_from_file(file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    println!(
        "{} Installing update from file: {}",
        "📦".bright_cyan(),
        file_path.bright_white().bold()
    );

    let result = proxy
        .install_update_from_file(file_path.to_string())
        .await?;

    println!("{} {}", "✓".bright_green().bold(), result.bright_white());

    Ok(())
}

async fn install_from_url(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    println!(
        "{} Downloading and installing update from: {}",
        "🌐".bright_cyan(),
        url.bright_white().bold()
    );

    let result = proxy.install_update_from_url(url.to_string()).await?;

    println!("{} {}", "✓".bright_green().bold(), result.bright_white());

    Ok(())
}

async fn get_pending_update() -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    let (version, changelog, source) = proxy.get_pending_update().await?;

    let border_top =
        "╔════════════════════════════════════════════════════════════════════════════╗";
    let border_mid =
        "╠════════════════════════════════════════════════════════════════════════════╣";
    let border_bot =
        "╚════════════════════════════════════════════════════════════════════════════╝";
    let side = "║";

    println!("{}", border_top.bright_cyan());
    println!(
        "{} {} {:<66} {}",
        side.bright_cyan(),
        "📦 PENDING UPDATE".bright_yellow().bold(),
        "",
        side.bright_cyan()
    );
    println!("{}", border_mid.bright_cyan());
    println!(
        "{} {} {:<57} {}",
        side.bright_cyan(),
        "Version:".bright_white().bold(),
        version.bright_green(),
        side.bright_cyan()
    );
    println!(
        "{} {} {:<58} {}",
        side.bright_cyan(),
        "Source:".bright_white().bold(),
        source.cyan(),
        side.bright_cyan()
    );
    println!("{}", border_mid.bright_cyan());
    println!(
        "{} {} {:<66} {}",
        side.bright_cyan(),
        "📝 CHANGELOG".bright_white().bold(),
        "",
        side.bright_cyan()
    );
    println!("{}", border_mid.bright_cyan());

    for line in changelog.lines() {
        let formatted_line = if line.starts_with("New Features:")
            || line.starts_with("Bug Fixes:")
            || line.starts_with("Performance:")
            || line.starts_with("Breaking Changes:")
        {
            line.bright_yellow().bold().to_string()
        } else if line.starts_with("- ") {
            format!("  {}", line.bright_white())
        } else {
            line.white().to_string()
        };
        println!(
            "{} {:<74} {}",
            side.bright_cyan(),
            formatted_line,
            side.bright_cyan()
        );
    }

    println!("{}", border_mid.bright_cyan());
    println!(
        "{} {} {:<41} {}",
        side.bright_cyan(),
        "Use".dimmed(),
        format!(
            "{} to install or {} to cancel",
            "embuer-client accept".bright_green(),
            "embuer-client reject".bright_red()
        ),
        side.bright_cyan()
    );
    println!("{}", border_bot.bright_cyan());

    Ok(())
}

async fn confirm_update(accepted: bool) -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    let result = proxy.confirm_update(accepted).await?;

    if accepted {
        println!("{} {}", "✅".bright_green(), result.bright_green().bold());
        println!("{}", "Installation will proceed...".bright_white());
    } else {
        println!("{} {}", "❌".bright_red(), result.bright_red().bold());
        println!("{}", "Update has been rejected.".bright_white());
    }

    Ok(())
}
