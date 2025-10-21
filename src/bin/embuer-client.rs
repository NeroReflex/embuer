use embuer::dbus::EmbuerDBusProxy;
use futures_util::StreamExt;
use std::process;
use zbus::Connection;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage(&args[0]);
        process::exit(1);
    }

    let command = &args[1];

    match command.as_str() {
        "status" => {
            if let Err(e) = get_status().await {
                eprintln!("Error getting status: {}", e);
                process::exit(1);
            }
        }
        "watch" => {
            if let Err(e) = watch_status().await {
                eprintln!("Error watching status: {}", e);
                process::exit(1);
            }
        }
        "install-file" => {
            if args.len() < 3 {
                eprintln!("Error: install-file requires a file path");
                print_usage(&args[0]);
                process::exit(1);
            }
            if let Err(e) = install_from_file(&args[2]).await {
                eprintln!("Error installing update: {}", e);
                process::exit(1);
            }
        }
        "install-url" => {
            if args.len() < 3 {
                eprintln!("Error: install-url requires a URL");
                print_usage(&args[0]);
                process::exit(1);
            }
            if let Err(e) = install_from_url(&args[2]).await {
                eprintln!("Error installing update: {}", e);
                process::exit(1);
            }
        }
        "help" | "--help" | "-h" => {
            print_usage(&args[0]);
        }
        _ => {
            eprintln!("Error: Unknown command '{}'", command);
            print_usage(&args[0]);
            process::exit(1);
        }
    }
}

fn print_usage(program_name: &str) {
    println!("Embuer Client - Control and monitor the Embuer update service");
    println!();
    println!("Usage: {} <command> [arguments]", program_name);
    println!();
    println!("Commands:");
    println!("  status              Get the current update status");
    println!("  watch               Watch for status changes in real-time");
    println!("  install-file <path> Install an update from a local file");
    println!("  install-url <url>   Install an update from a URL");
    println!("  help                Show this help message");
    println!();
    println!("Examples:");
    println!("  {} status", program_name);
    println!("  {} install-file /path/to/update.tar.gz", program_name);
    println!(
        "  {} install-url https://example.com/update.tar.gz",
        program_name
    );
}

async fn get_connection() -> Result<Connection, Box<dyn std::error::Error>> {
    let connection = Connection::system().await?;
    Ok(connection)
}

async fn get_status() -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    let (status, details, progress) = proxy.get_update_status().await?;

    println!("Status: {}", status);
    println!("Details: {}", details);
    if progress >= 0 {
        println!("Progress: {}%", progress);
    } else {
        println!("Progress: N/A");
    }

    Ok(())
}

async fn watch_status() -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    println!("Watching for status updates... (Ctrl+C to exit)");
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
    let timestamp = chrono::Local::now().format("%H:%M:%S");
    if progress >= 0 {
        println!("[{}] {} - {} ({}%)", timestamp, status, details, progress);
    } else {
        println!("[{}] {} - {}", timestamp, status, details);
    }
}

async fn install_from_file(file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    let result = proxy
        .install_update_from_file(file_path.to_string())
        .await?;
    println!("{}", result);

    Ok(())
}

async fn install_from_url(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let connection = get_connection().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    let result = proxy.install_update_from_url(url.to_string()).await?;
    println!("{}", result);

    Ok(())
}
