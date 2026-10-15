//! `net-probe`: isolated cross-NAT probe for Pastey research. See README.md.

use std::{path::PathBuf, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use pastey_net_probe::{
    bind_endpoint, decode_ticket, dial, encode_ticket, load_or_create_key, serve, Allowlist,
    DialPlan, HostInfo, NetOptions, Relay,
};

const USAGE: &str = "\
net-probe id     --key FILE
net-probe listen --key FILE (--allow ID[,ID...] | --allow-any) [net options]
net-probe dial   TICKET --key FILE [--cell ID] [--bulk-mib N] [--http]
                 [--watch SECS] [--direct-wait SECS] [net options]
net-probe local  [--bulk-mib N] [--http] [--relay-only-listener] [net options]
                 (two endpoints in this process: never cross-NAT evidence)

net options: [--relay n0|none|https://relay.example] [--no-dns] [--relay-only]
             [--no-portmap] [--net \"free text describing this network\"]";

struct Args {
    rest: Vec<String>,
}

impl Args {
    fn flag(&mut self, name: &str) -> bool {
        if let Some(index) = self.rest.iter().position(|arg| arg == name) {
            self.rest.remove(index);
            true
        } else {
            false
        }
    }

    fn value(&mut self, name: &str) -> Result<Option<String>> {
        let Some(index) = self.rest.iter().position(|arg| arg == name) else {
            return Ok(None);
        };
        if index + 1 >= self.rest.len() {
            bail!("{name} needs a value");
        }
        let value = self.rest.remove(index + 1);
        self.rest.remove(index);
        Ok(Some(value))
    }

    fn net_options(&mut self) -> Result<(NetOptions, String)> {
        let relay = match self.value("--relay")?.as_deref() {
            None | Some("n0") => Relay::N0,
            Some("none") => Relay::Disabled,
            Some(url) => Relay::Custom(url.parse().context("--relay URL")?),
        };
        let options = NetOptions {
            relay,
            dns: !self.flag("--no-dns"),
            ip: !self.flag("--relay-only"),
            portmapper: !self.flag("--no-portmap"),
            insecure_relay_tls: false,
        };
        let net = self.value("--net")?.unwrap_or_else(|| "unspecified".into());
        Ok((options, net))
    }

    fn key(&mut self) -> Result<PathBuf> {
        self.value("--key")?
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("--key FILE is required"))
    }

    fn finish(self) -> Result<()> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            bail!("unexpected arguments: {:?}\n\n{USAGE}", self.rest)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut raw = std::env::args().skip(1).collect::<Vec<_>>();
    if raw.is_empty() || raw[0] == "--help" || raw[0] == "-h" {
        println!("{USAGE}");
        return Ok(());
    }
    let command = raw.remove(0);
    let mut args = Args { rest: raw };
    match command.as_str() {
        "id" => {
            let key = load_or_create_key(&args.key()?)?;
            args.finish()?;
            println!("{}", key.public());
        }
        "listen" => listen(args).await?,
        "dial" => dial_command(args).await?,
        "local" => local(args).await?,
        other => bail!("unknown command {other}\n\n{USAGE}"),
    }
    Ok(())
}

async fn listen(mut args: Args) -> Result<()> {
    let key = load_or_create_key(&args.key()?)?;
    let allowlist = if args.flag("--allow-any") {
        eprintln!("[net-probe] WARNING: --allow-any accepts every key");
        Allowlist::any()
    } else {
        let ids = args
            .value("--allow")?
            .ok_or_else(|| anyhow!("--allow ID[,ID...] or --allow-any is required"))?;
        Allowlist::only(
            ids.split(',')
                .map(|id| id.trim().parse().context("--allow EndpointId"))
                .collect::<Result<Vec<_>>>()?,
        )
    };
    let (options, net) = args.net_options()?;
    args.finish()?;
    let endpoint = bind_endpoint(&options, key, Some(allowlist)).await?;
    if !matches!(options.relay, Relay::Disabled) {
        let _ = tokio::time::timeout(Duration::from_secs(15), endpoint.online()).await;
    }
    // Give local address discovery a moment so the ticket carries them.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let info = HostInfo::local(&endpoint, &net);
    eprintln!("[net-probe] EndpointId {}", endpoint.id());
    println!("{}", encode_ticket(&endpoint.addr())?);
    eprintln!("[net-probe] listening; Ctrl-C to stop");
    tokio::select! {
        _ = serve(endpoint.clone(), info) => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    endpoint.close().await;
    Ok(())
}

fn plan(args: &mut Args) -> Result<DialPlan> {
    let mut plan = DialPlan::default();
    if let Some(value) = args.value("--bulk-mib")? {
        plan.bulk_mib = value.parse().context("--bulk-mib")?;
    }
    if let Some(value) = args.value("--watch")? {
        plan.watch = Duration::from_secs(value.parse().context("--watch")?);
    }
    if let Some(value) = args.value("--direct-wait")? {
        plan.direct_wait = Duration::from_secs(value.parse().context("--direct-wait")?);
    }
    plan.http = args.flag("--http");
    Ok(plan)
}

async fn dial_command(mut args: Args) -> Result<()> {
    if args.rest.is_empty() {
        bail!("dial needs a TICKET\n\n{USAGE}");
    }
    let ticket = args.rest.remove(0);
    let addr = decode_ticket(&ticket)?;
    let key = load_or_create_key(&args.key()?)?;
    let cell = args.value("--cell")?.unwrap_or_default();
    let plan = plan(&mut args)?;
    let (options, net) = args.net_options()?;
    args.finish()?;
    let endpoint = bind_endpoint(&options, key, None).await?;
    let mut report = dial(&endpoint, addr, &plan, &net, &options).await;
    report.cell = cell;
    println!("{}", serde_json::to_string(&report)?);
    endpoint.close().await;
    Ok(())
}

/// Both endpoints in this process, through the chosen relays. Useful for a
/// smoke test of real relay reachability from this network; a direct path
/// here is a local path.
async fn local(mut args: Args) -> Result<()> {
    let plan = plan(&mut args)?;
    let cell = args.value("--cell")?.unwrap_or_else(|| "S0".into());
    let relay_only_listener = args.flag("--relay-only-listener");
    let (options, net) = args.net_options()?;
    args.finish()?;
    let dialer_key = iroh::SecretKey::generate();
    let listener_options = NetOptions {
        ip: options.ip && !relay_only_listener,
        ..options.clone()
    };
    let listener = bind_endpoint(
        &listener_options,
        iroh::SecretKey::generate(),
        Some(Allowlist::only([dialer_key.public()])),
    )
    .await?;
    let dialer = bind_endpoint(&options, dialer_key, None).await?;
    if !matches!(options.relay, Relay::Disabled) {
        tokio::time::timeout(Duration::from_secs(20), listener.online())
            .await
            .context("listener did not reach its home relay")?;
    }
    let info = HostInfo::local(&listener, &net);
    let server = tokio::spawn(serve(listener.clone(), info));
    let mut report = dial(&dialer, listener.addr(), &plan, &net, &options).await;
    report.single_machine = true;
    report.cell = cell;
    println!("{}", serde_json::to_string(&report)?);
    dialer.close().await;
    listener.close().await;
    server.abort();
    Ok(())
}
