//! `aura login`, `aura logout`, `aura whoami` and `aura auth`.

use std::io::Write;

use anyhow::Context;
use aura_auth::{
    sign_in_notice, AuthManager, LinkMode, LinkStatus, LinkedAccount, Owner, ProviderKind,
};
use colored::Colorize;

use crate::cli::{AuthCommand, AuthSubcommand, LoginArgs, LogoutArgs};

fn manager() -> anyhow::Result<AuthManager> {
    AuthManager::new().context("loading sign-in state")
}

fn parse_provider(raw: &str) -> anyhow::Result<ProviderKind> {
    let kind: ProviderKind = raw.parse()?;
    if kind == ProviderKind::Local {
        anyhow::bail!("`local` needs no sign-in; use zos, claude or chatgpt");
    }
    Ok(kind)
}

pub(crate) async fn cmd_login(args: LoginArgs) -> anyhow::Result<()> {
    let kind = parse_provider(&args.provider)?;
    let auth = manager()?;
    if kind == ProviderKind::Zos {
        return login_zos(&auth).await;
    }
    link_plan(&auth, kind, args.paste).await
}

async fn login_zos(auth: &AuthManager) -> anyhow::Result<()> {
    print!("Email: ");
    std::io::stdout().flush()?;
    let mut email = String::new();
    std::io::stdin().read_line(&mut email)?;
    let email = email.trim();
    if email.is_empty() {
        anyhow::bail!("Email cannot be empty");
    }
    let password = rpassword::prompt_password("Password: ")?;
    if password.is_empty() {
        anyhow::bail!("Password cannot be empty");
    }

    println!("{} Authenticating...", "▶".blue().bold());
    let session = auth.login_zos(email, &password).await?;
    println!(
        "{} Logged in as {} ({})",
        "✓".green().bold(),
        session.display_name.green(),
        session.primary_zid,
    );
    let local = auth.vault().links_for(&Owner::Local);
    if !local.is_empty() {
        let names: Vec<&str> = local.iter().map(|l| l.provider.label()).collect();
        println!(
            "{} {} linked while signed out. Run `aura auth move --to-zos` to attach {} to this account.",
            "ℹ".blue().bold(),
            names.join(" and "),
            if local.len() == 1 { "it" } else { "them" },
        );
    }
    Ok(())
}

async fn link_plan(auth: &AuthManager, kind: ProviderKind, paste: bool) -> anyhow::Result<()> {
    if let Some(notice) = sign_in_notice(kind) {
        println!("{} {notice}", "!".yellow().bold());
    }
    let mode = if paste {
        LinkMode::Paste
    } else {
        LinkMode::Browser
    };
    let pending = auth.begin_link(kind, mode).await?;
    let url = pending.url().to_string();
    let opened = !pending.needs_paste() && aura_auth::open_browser(&url);
    println!(
        "{} {} to sign in to {}:\n\n  {url}\n",
        "▶".blue().bold(),
        if opened {
            "Opened your browser"
        } else {
            "Open this URL"
        },
        kind.label(),
    );

    let link = if pending.needs_paste() {
        print!("Paste the code shown after sign-in: ");
        std::io::stdout().flush()?;
        let mut code = String::new();
        std::io::stdin().read_line(&mut code)?;
        auth.finish_link(pending, Some(code.trim())).await?
    } else {
        println!(
            "Waiting for the browser to finish (Ctrl+C to cancel; on another machine use `aura login {} --paste`)...",
            kind.id()
        );
        auth.finish_link(pending, None).await?
    };

    println!(
        "{} {} linked to {}",
        "✓".green().bold(),
        kind.label(),
        owner_label(&auth.owner()),
    );
    println!("  {}", describe_link(&link));
    if link.plan_enabled {
        println!(
            "  Pick a `{}/…` model with /model; those turns bill your plan, not Aura.",
            kind.id()
        );
    }
    Ok(())
}

pub(crate) async fn cmd_logout(args: LogoutArgs) -> anyhow::Result<()> {
    let auth = manager()?;
    if args.all {
        for link in auth.links() {
            auth.unlink(link.provider).await?;
            println!("{} {} unlinked", "✓".green().bold(), link.provider.label());
        }
        return logout_zos(&auth).await;
    }
    let kind = parse_provider(&args.provider)?;
    if kind == ProviderKind::Zos {
        return logout_zos(&auth).await;
    }
    match auth.unlink(kind).await? {
        Some(_) => println!("{} {} unlinked", "✓".green().bold(), kind.label()),
        None => println!("{} No {} plan is linked", "ℹ".blue().bold(), kind.label()),
    }
    Ok(())
}

async fn logout_zos(auth: &AuthManager) -> anyhow::Result<()> {
    let had_links = !auth.links().is_empty();
    auth.logout_zos().await?;
    println!("{} Logged out", "✓".green().bold());
    if had_links {
        println!("  Linked plans stay with your zOS account and return when you log in again.");
    }
    Ok(())
}

pub(crate) fn cmd_whoami() -> anyhow::Result<()> {
    let auth = manager()?;
    println!("{}", "Authentication".cyan().bold());
    match auth.vault().zos_session() {
        Some(session) => {
            println!("  Name:    {}", session.display_name);
            println!("  zID:     {}", session.primary_zid);
            println!("  User ID: {}", session.user_id);
            println!(
                "  Since:   {}",
                session.created_at.format("%Y-%m-%d %H:%M UTC")
            );
        }
        None => println!("  Not logged in to zOS. Run `aura login` for cloud models."),
    }
    println!("{}", "Plans".cyan().bold());
    for kind in auth.subscription_kinds() {
        match auth.link(kind) {
            Some(link) => println!("  {}", describe_link(&link)),
            None => println!(
                "  {}: not linked. Run `aura login {}`.",
                kind.label(),
                kind.id()
            ),
        }
    }
    Ok(())
}

pub(crate) fn cmd_auth(command: AuthCommand) -> anyhow::Result<()> {
    match command.action {
        AuthSubcommand::Move { to_zos: _ } => {
            let auth = manager()?;
            let moved = auth.move_local_links_to_zos()?;
            println!(
                "{} Moved {moved} plan link{} to {}",
                "✓".green().bold(),
                if moved == 1 { "" } else { "s" },
                owner_label(&auth.owner()),
            );
            Ok(())
        }
    }
}

fn owner_label(owner: &Owner) -> String {
    match owner {
        Owner::Local => "this machine (not signed in to zOS)".to_string(),
        Owner::Zos { .. } => aura_auth::CredentialStore::load().map_or_else(
            || "your zOS account".to_string(),
            |s| format!("zOS account {}", s.display_name),
        ),
    }
}

fn describe_link(link: &LinkedAccount) -> String {
    let account = if link.label.is_empty() {
        String::new()
    } else {
        format!(" as {}", link.label)
    };
    let status = match &link.status {
        LinkStatus::Active if link.plan_enabled => "active".to_string(),
        LinkStatus::Active => "signed in, but the plan is not enabled for this app".to_string(),
        LinkStatus::ReauthRequired { reason } => {
            format!("run `aura login {}` again ({reason})", link.provider.id())
        }
    };
    format!("{}: linked{account} — {status}", link.provider.label())
}
