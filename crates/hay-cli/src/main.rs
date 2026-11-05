//! `hay` — the Haystack command line.
//!
//! ```text
//! hay convert site.zinc --to json3            # any format -> any format
//! hay read  http://host/api/demo "point and his" --user admin --password …
//! hay his   http://host/api/demo @p1 today
//! hay serve site.trio --addr 127.0.0.1:8080   # instant Haystack server
//! ```

use clap::{Parser, Subcommand};
use libhaystack::val::{Grid, Ref};
use openhay_client::Client;
use openhay_codec::{Format, decode_grid, decode_grid_auto, encode_grid};
use openhay_server::{Config, MemBackend, Server};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "hay",
    version,
    about = "Haystack 3/4 toolkit: convert, query, serve"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args)]
struct Conn {
    /// API root, e.g. http://host/api/demo (SkySpark) or http://host/api (Haxall)
    url: String,
    #[arg(long, env = "HAYSTACK_USER")]
    user: Option<String>,
    #[arg(long, env = "HAYSTACK_PASSWORD")]
    password: Option<String>,
    /// Output format
    #[arg(long, default_value = "zinc")]
    to: String,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert a grid between zinc, json3, hayson, trio and csv
    Convert {
        /// Input file ("-" for stdin)
        input: String,
        /// Input format (default: sniff)
        #[arg(long)]
        from: Option<String>,
        #[arg(long, default_value = "hayson")]
        to: String,
    },
    /// Read records by filter
    Read {
        #[command(flatten)]
        conn: Conn,
        filter: String,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Read history for one point
    His {
        #[command(flatten)]
        conn: Conn,
        id: String,
        #[arg(default_value = "today")]
        range: String,
    },
    /// Call any op with an empty request
    Call {
        #[command(flatten)]
        conn: Conn,
        op: String,
    },
    /// Serve records from a Trio/Zinc/JSON file over the Haystack HTTP API
    Serve {
        file: String,
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: String,
        /// Require SCRAM login as user:password
        #[arg(long)]
        auth: Option<String>,
    },
}

fn fmt(name: &str) -> Result<Format, String> {
    Format::from_name(name)
        .ok_or_else(|| format!("unknown format {name:?} (zinc, json3, hayson, trio, csv)"))
}

fn read_input(path: &str) -> std::io::Result<String> {
    if path == "-" {
        std::io::read_to_string(std::io::stdin())
    } else {
        std::fs::read_to_string(path)
    }
}

async fn connect(c: &Conn) -> Result<Client, Box<dyn std::error::Error>> {
    let mut client = Client::new(&c.url);
    if let (Some(u), Some(p)) = (&c.user, &c.password) {
        client.login(u, p).await?;
    }
    Ok(client)
}

fn print(grid: &Grid, to: &str) -> Result<(), Box<dyn std::error::Error>> {
    print!("{}", encode_grid(fmt(to)?, grid)?);
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = run(Cli::parse()).await {
        eprintln!("hay: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.cmd {
        Cmd::Convert { input, from, to } => {
            let text = read_input(&input)?;
            let grid = match from {
                Some(f) => decode_grid(fmt(&f)?, &text)?,
                None => decode_grid_auto(&text)?,
            };
            print(&grid, &to)?;
        }
        Cmd::Read {
            conn,
            filter,
            limit,
        } => {
            let c = connect(&conn).await?;
            let rows = c.read(&filter, limit).await?;
            let g = if rows.is_empty() {
                Grid::make_empty()
            } else {
                Grid::make_from_dicts(rows)
            };
            print(&g, &conn.to)?;
        }
        Cmd::His { conn, id, range } => {
            let c = connect(&conn).await?;
            let g = c
                .his_read(&Ref::make(id.trim_start_matches('@'), None), &range)
                .await?;
            print(&g, &conn.to)?;
        }
        Cmd::Call { conn, op } => {
            let c = connect(&conn).await?;
            print(&c.call(&op, &Grid::make_empty()).await?, &conn.to)?;
        }
        Cmd::Serve { file, addr, auth } => {
            let grid = decode_grid_auto(&read_input(&file)?)?;
            let n = grid.rows.len();
            let users = match auth {
                Some(a) => {
                    let (u, p) = a.split_once(':').ok_or("--auth must be user:password")?;
                    let mut m = HashMap::new();
                    m.insert(u.to_string(), openhay_auth_creds(p));
                    Some(m)
                }
                None => None,
            };
            let server = Arc::new(Server::new(
                MemBackend::new(grid.rows),
                Config {
                    users,
                    ..Config::default()
                },
            ));
            eprintln!("hay: serving {n} records at http://{addr}/api/ (and /api/<project>/)");
            openhay_server::serve(server, &addr).await?;
        }
    }
    Ok(())
}

fn openhay_auth_creds(password: &str) -> openhay_server::ScramCredentials {
    openhay_server::ScramCredentials::new_random(password)
}
