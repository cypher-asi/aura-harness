use colored::Colorize;

/// Print the available commands and their descriptions.
pub(crate) fn print_help() {
    println!("{}", "Available Commands".cyan().bold());
    println!();
    println!("  {}    Submit a prompt to the agent", "<text>".green());
    println!("  {}   Show agent status", "/status".green());
    println!("  {} Show last N history entries", "/history [n]".green());
    println!("  {}  Approve pending tool request", "/approve".green());
    println!("  {}     Deny pending tool request", "/deny".green());
    println!("  {}     Show pending file changes", "/diff".green());
    println!("  {}    Login to zOS for proxy mode", "/login".green());
    println!("  {}   Clear stored credentials", "/logout".green());
    println!("  {}   Show auth status", "/whoami".green());
    println!("  {}     Show this help message", "/help".green());
    println!("  {}     Exit the CLI", "/quit".green());
    println!();
    println!("  Shortcuts: /s, /h, /y, /n, /d, /me, /?, /q");
    println!();
}

pub(crate) fn banner() -> String {
    format!(
        r"
{}
Version: {}
Type /help for available commands.
",
        r"
    _   _   _ ____      _    
   / \ | | | |  _ \    / \   
  / _ \| | | | |_) |  / _ \  
 / ___ \ |_| |  _ <  / ___ \ 
/_/   \_\___/|_| \_\/_/   \_\
"
        .cyan()
        .bold(),
        env!("CARGO_PKG_VERSION")
    )
}
