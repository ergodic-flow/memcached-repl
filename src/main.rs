use std::env;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command as ProcessCommand, Stdio};

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Context, Editor, Helper};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 11211;
const DEFAULT_TERMINAL_ROWS: usize = 24;
const REPL_COMMANDS: &[&str] = &[
    "GET",
    "GETS",
    "SET",
    "ADD",
    "REPLACE",
    "APPEND",
    "PREPEND",
    "CAS",
    "DELETE",
    "INCR",
    "DECR",
    "TOUCH",
    "FLUSHALL",
    "FLUSH_ALL",
    "CACHEDUMP",
    "CACHE_DUMP",
    "STATS",
    "VERSION",
    "HELP",
    "QUIT",
    "EXIT",
];

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli_action = parse_cli_args(env::args().skip(1))
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let CliAction::Run(config) = cli_action else {
        print_cli_usage();
        return Ok(());
    };

    let stream = TcpStream::connect((config.host.as_str(), config.port))?;
    let mut client = MemcacheClient::new(stream)?;

    let addr = config.display_addr();
    println!("connected to {addr}");
    println!(
        "commands: GET KEY | SET KEY VALUE EXPIRES_IN_SECONDS | DELETE KEY | CACHEDUMP SLAB_ID LIMIT | STATS | HELP | QUIT"
    );

    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut editor = Editor::<ReplHelper, DefaultHistory>::with_config(config)?;
    editor.set_helper(Some(ReplHelper));
    let history_path = history_path();
    if let Some(path) = &history_path {
        let _ = editor.load_history(path);
    }

    loop {
        let input = match editor.readline("> ") {
            Ok(input) => input,
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(err) => return Err(Box::new(err)),
        };

        if !input.trim().is_empty() {
            let _ = editor.add_history_entry(input.as_str());
        }

        match parse_command(input.trim()) {
            Ok(Command::Get { key }) => match client.get(&key) {
                Ok(Some(value)) => println!("{key} = {value}"),
                Ok(None) => println!("(nil)"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Set {
                key,
                value,
                expires_in_seconds,
            }) => match client.set(&key, &value, expires_in_seconds) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Add {
                key,
                value,
                expires_in_seconds,
            }) => match client.add(&key, &value, expires_in_seconds) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Replace {
                key,
                value,
                expires_in_seconds,
            }) => match client.replace(&key, &value, expires_in_seconds) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Append { key, value }) => match client.append(&key, &value) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Prepend { key, value }) => match client.prepend(&key, &value) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Gets { key }) => match client.gets(&key) {
                Ok(Some(item)) => {
                    println!("{} = {} (cas: {})", item.key, item.value, item.cas_id())
                }
                Ok(None) => println!("(nil)"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Cas {
                key,
                value,
                expires_in_seconds,
                cas_id,
            }) => match client.cas(&key, &value, expires_in_seconds, cas_id) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Delete { key }) => match client.delete(&key) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Incr { key, amount }) => match client.incr(&key, amount) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Decr { key, amount }) => match client.decr(&key, amount) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Touch {
                key,
                expires_in_seconds,
            }) => match client.touch(&key, expires_in_seconds) {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::FlushAll { expires_in_seconds }) => {
                match client.flush_all(expires_in_seconds) {
                    Ok(response) => println!("{response}"),
                    Err(err) => eprintln!("error: {err}"),
                }
            }
            Ok(Command::CacheDump { slab_id, limit }) => match client.cachedump(slab_id, limit) {
                Ok(lines) => print_response_lines(&lines),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Stats { kind }) => match client.stats(kind) {
                Ok(lines) => print_response_lines(&lines),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Version) => match client.version() {
                Ok(response) => println!("{response}"),
                Err(err) => eprintln!("error: {err}"),
            },
            Ok(Command::Help) => print_help(),
            Ok(Command::Quit) => break,
            Ok(Command::Empty) => {}
            Err(err) => eprintln!("error: {err}"),
        }
    }

    if let Some(path) = &history_path {
        let _ = editor.save_history(path);
    }

    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum CliAction {
    Run(CliConfig),
    Help,
}

#[derive(Debug, PartialEq, Eq)]
struct CliConfig {
    host: String,
    port: u16,
}

impl CliConfig {
    fn display_addr(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

fn parse_cli_args<I, S>(args: I) -> Result<CliAction, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut host = DEFAULT_HOST.to_string();
    let mut port = DEFAULT_PORT;
    let mut args = args.into_iter().map(Into::into).peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(CliAction::Help),
            "--host" => host = parse_host_arg(&required_cli_arg_value(&mut args, "--host")?)?,
            "--port" => port = parse_port_arg(&required_cli_arg_value(&mut args, "--port")?)?,
            _ if arg.starts_with("--host=") => {
                host = parse_host_arg(arg.trim_start_matches("--host="))?
            }
            _ if arg.starts_with("--port=") => {
                port = parse_port_arg(arg.trim_start_matches("--port="))?
            }
            _ => return Err(format!("unexpected argument: {arg}\n{}", cli_usage())),
        }
    }

    Ok(CliAction::Run(CliConfig { host, port }))
}

fn required_cli_arg_value<I>(
    args: &mut std::iter::Peekable<I>,
    flag: &str,
) -> Result<String, String>
where
    I: Iterator<Item = String>,
{
    match args.next() {
        Some(value) if !value.starts_with("--") => Ok(value),
        Some(value) => Err(format!("{flag} requires a value; found {value}")),
        None => Err(format!("{flag} requires a value")),
    }
}

fn parse_host_arg(host: &str) -> Result<String, String> {
    if host.is_empty() {
        return Err("--host cannot be empty".to_string());
    }

    Ok(host.to_string())
}

fn parse_port_arg(port: &str) -> Result<u16, String> {
    let port = port
        .parse::<u16>()
        .map_err(|_| "--port must be a number between 1 and 65535".to_string())?;
    if port == 0 {
        return Err("--port must be a number between 1 and 65535".to_string());
    }

    Ok(port)
}

fn cli_usage() -> &'static str {
    "usage: memcache-cli [--host HOST] [--port PORT]"
}

fn print_cli_usage() {
    println!("{}", cli_usage());
    println!();
    println!("options:");
    println!("  --host HOST    Memcache server host (default: {DEFAULT_HOST})");
    println!("  --port PORT    Memcache server port (default: {DEFAULT_PORT})");
    println!("  -h, --help     Show this help");
}

fn history_path() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".memcache-cli-history"))
}

struct ReplHelper;

impl Helper for ReplHelper {}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        Ok(repl_command_completions(line, pos))
    }
}

impl Hinter for ReplHelper {
    type Hint = String;
}

impl Highlighter for ReplHelper {}

impl Validator for ReplHelper {}

fn repl_command_completions(line: &str, pos: usize) -> (usize, Vec<Pair>) {
    let Some(before_cursor) = line.get(..pos) else {
        return (0, Vec::new());
    };

    let start = before_cursor
        .char_indices()
        .find_map(|(idx, ch)| (!ch.is_whitespace()).then_some(idx))
        .unwrap_or(pos);
    let prefix = &before_cursor[start..];
    if prefix.chars().any(char::is_whitespace) {
        return (start, Vec::new());
    }

    let prefix = prefix.to_ascii_uppercase();
    let commands: Vec<_> = if REPL_COMMANDS.contains(&prefix.as_str()) {
        vec![prefix.as_str()]
    } else {
        REPL_COMMANDS
            .iter()
            .copied()
            .filter(|command| command.starts_with(&prefix))
            .collect()
    };
    let completions = commands
        .into_iter()
        .map(|command| Pair {
            display: command.to_string(),
            replacement: format!("{command} "),
        })
        .collect();

    (start, completions)
}

struct MemcacheClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
}

struct CacheItem {
    key: String,
    value: String,
    cas_id: Option<u64>,
}

impl CacheItem {
    fn cas_id(&self) -> u64 {
        self.cas_id.unwrap_or(0)
    }
}

struct ValueHeader {
    key: String,
    byte_count: usize,
    cas_id: Option<u64>,
}

impl MemcacheClient {
    fn new(stream: TcpStream) -> io::Result<Self> {
        let writer = stream.try_clone()?;

        Ok(Self {
            reader: BufReader::new(stream),
            writer,
        })
    }

    fn get(&mut self, key: &str) -> io::Result<Option<String>> {
        writeln!(self.writer, "get {key}\r")?;
        self.writer.flush()?;

        Ok(self.read_single_item()?.map(|item| item.value))
    }

    fn gets(&mut self, key: &str) -> io::Result<Option<CacheItem>> {
        writeln!(self.writer, "gets {key}\r")?;
        self.writer.flush()?;
        self.read_single_item()
    }

    fn set(&mut self, key: &str, value: &str, expires_in_seconds: u32) -> io::Result<String> {
        self.storage_command("set", key, value, expires_in_seconds, None)
    }

    fn add(&mut self, key: &str, value: &str, expires_in_seconds: u32) -> io::Result<String> {
        self.storage_command("add", key, value, expires_in_seconds, None)
    }

    fn replace(&mut self, key: &str, value: &str, expires_in_seconds: u32) -> io::Result<String> {
        self.storage_command("replace", key, value, expires_in_seconds, None)
    }

    fn append(&mut self, key: &str, value: &str) -> io::Result<String> {
        self.storage_command("append", key, value, 0, None)
    }

    fn prepend(&mut self, key: &str, value: &str) -> io::Result<String> {
        self.storage_command("prepend", key, value, 0, None)
    }

    fn cas(
        &mut self,
        key: &str,
        value: &str,
        expires_in_seconds: u32,
        cas_id: u64,
    ) -> io::Result<String> {
        self.storage_command("cas", key, value, expires_in_seconds, Some(cas_id))
    }

    fn storage_command(
        &mut self,
        command: &str,
        key: &str,
        value: &str,
        expires_in_seconds: u32,
        cas_id: Option<u64>,
    ) -> io::Result<String> {
        write!(
            self.writer,
            "{command} {key} 0 {expires_in_seconds} {}",
            value.len()
        )?;

        if let Some(cas_id) = cas_id {
            write!(self.writer, " {cas_id}")?;
        }

        write!(self.writer, "\r\n{value}\r\n")?;
        self.writer.flush()?;
        self.read_response_line()
    }

    fn read_single_item(&mut self) -> io::Result<Option<CacheItem>> {
        let mut item = None;

        loop {
            let line = self.read_response_line()?;
            if line == "END" {
                return Ok(item);
            }

            if line.starts_with("VALUE ") {
                let header = parse_value_header(&line)?;
                let mut bytes = vec![0; header.byte_count];
                self.reader.read_exact(&mut bytes)?;

                let mut crlf = [0; 2];
                self.reader.read_exact(&mut crlf)?;
                if crlf != *b"\r\n" {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "memcache response missing value terminator",
                    ));
                }

                item = Some(CacheItem {
                    key: header.key,
                    value: String::from_utf8_lossy(&bytes).into_owned(),
                    cas_id: header.cas_id,
                });
                continue;
            }

            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected memcache response: {line}"),
            ));
        }
    }

    fn delete(&mut self, key: &str) -> io::Result<String> {
        writeln!(self.writer, "delete {key}\r")?;
        self.writer.flush()?;
        self.read_response_line()
    }

    fn incr(&mut self, key: &str, amount: u64) -> io::Result<String> {
        writeln!(self.writer, "incr {key} {amount}\r")?;
        self.writer.flush()?;
        self.read_response_line()
    }

    fn decr(&mut self, key: &str, amount: u64) -> io::Result<String> {
        writeln!(self.writer, "decr {key} {amount}\r")?;
        self.writer.flush()?;
        self.read_response_line()
    }

    fn touch(&mut self, key: &str, expires_in_seconds: u32) -> io::Result<String> {
        writeln!(self.writer, "touch {key} {expires_in_seconds}\r")?;
        self.writer.flush()?;
        self.read_response_line()
    }

    fn flush_all(&mut self, expires_in_seconds: Option<u32>) -> io::Result<String> {
        match expires_in_seconds {
            Some(seconds) => writeln!(self.writer, "flush_all {seconds}\r")?,
            None => writeln!(self.writer, "flush_all\r")?,
        }
        self.writer.flush()?;
        self.read_response_line()
    }

    fn stats(&mut self, kind: StatsKind) -> io::Result<Vec<String>> {
        match kind {
            StatsKind::Default => writeln!(self.writer, "stats\r")?,
            StatsKind::Cache => writeln!(self.writer, "stats items\r")?,
            StatsKind::Slab => writeln!(self.writer, "stats slabs\r")?,
            StatsKind::Sizes => writeln!(self.writer, "stats sizes\r")?,
        }
        self.writer.flush()?;
        self.read_lines_until_end()
    }

    fn cachedump(&mut self, slab_id: u32, limit: u32) -> io::Result<Vec<String>> {
        writeln!(self.writer, "stats cachedump {slab_id} {limit}\r")?;
        self.writer.flush()?;
        self.read_lines_until_end()
    }

    fn version(&mut self) -> io::Result<String> {
        writeln!(self.writer, "version\r")?;
        self.writer.flush()?;
        self.read_response_line()
    }

    fn read_response_line(&mut self) -> io::Result<String> {
        let mut line = String::new();
        let bytes = self.reader.read_line(&mut line)?;
        if bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "memcache server closed the connection",
            ));
        }

        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    fn read_lines_until_end(&mut self) -> io::Result<Vec<String>> {
        let mut lines = Vec::new();

        loop {
            let line = self.read_response_line()?;
            if line == "END" {
                return Ok(lines);
            }

            if line == "ERROR"
                || line.starts_with("CLIENT_ERROR")
                || line.starts_with("SERVER_ERROR")
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("memcache returned {line}"),
                ));
            }

            lines.push(line);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Get {
        key: String,
    },
    Set {
        key: String,
        value: String,
        expires_in_seconds: u32,
    },
    Add {
        key: String,
        value: String,
        expires_in_seconds: u32,
    },
    Replace {
        key: String,
        value: String,
        expires_in_seconds: u32,
    },
    Append {
        key: String,
        value: String,
    },
    Prepend {
        key: String,
        value: String,
    },
    Gets {
        key: String,
    },
    Cas {
        key: String,
        value: String,
        expires_in_seconds: u32,
        cas_id: u64,
    },
    Delete {
        key: String,
    },
    Incr {
        key: String,
        amount: u64,
    },
    Decr {
        key: String,
        amount: u64,
    },
    Touch {
        key: String,
        expires_in_seconds: u32,
    },
    FlushAll {
        expires_in_seconds: Option<u32>,
    },
    CacheDump {
        slab_id: u32,
        limit: u32,
    },
    Stats {
        kind: StatsKind,
    },
    Version,
    Help,
    Quit,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatsKind {
    Default,
    Cache,
    Slab,
    Sizes,
}

fn parse_command(input: &str) -> Result<Command, String> {
    let mut parts = input.split_whitespace();
    let Some(command) = parts.next() else {
        return Ok(Command::Empty);
    };

    match command.to_ascii_uppercase().as_str() {
        "GET" => {
            let key = required_part(&mut parts, "GET requires KEY")?;
            reject_extra_parts(parts, "usage: GET KEY")?;
            validate_key(key)?;
            Ok(Command::Get {
                key: key.to_string(),
            })
        }
        "GETS" => {
            let key = required_part(&mut parts, "GETS requires KEY")?;
            reject_extra_parts(parts, "usage: GETS KEY")?;
            validate_key(key)?;
            Ok(Command::Gets {
                key: key.to_string(),
            })
        }
        "SET" => parse_storage_with_expiration(parts, "SET"),
        "ADD" => parse_storage_with_expiration(parts, "ADD"),
        "REPLACE" => parse_storage_with_expiration(parts, "REPLACE"),
        "APPEND" => parse_storage_value(parts, "APPEND"),
        "PREPEND" => parse_storage_value(parts, "PREPEND"),
        "CAS" => parse_cas(parts),
        "DELETE" => {
            let key = required_part(&mut parts, "DELETE requires KEY")?;
            reject_extra_parts(parts, "usage: DELETE KEY")?;
            validate_key(key)?;
            Ok(Command::Delete {
                key: key.to_string(),
            })
        }
        "INCR" => parse_counter(parts, "INCR"),
        "DECR" => parse_counter(parts, "DECR"),
        "TOUCH" => parse_touch(parts),
        "FLUSHALL" | "FLUSH_ALL" => {
            let expires_in_seconds =
                optional_expires_in_seconds(parts, "usage: FLUSHALL [EXPIRES_IN_SECONDS]")?;
            Ok(Command::FlushAll { expires_in_seconds })
        }
        "CACHEDUMP" | "CACHE_DUMP" => parse_cachedump(parts),
        "STATS" => parse_stats(parts),
        "VERSION" => {
            reject_extra_parts(parts, "usage: VERSION")?;
            Ok(Command::Version)
        }
        "HELP" => {
            reject_extra_parts(parts, "usage: HELP")?;
            Ok(Command::Help)
        }
        "QUIT" | "EXIT" => {
            reject_extra_parts(parts, "usage: QUIT")?;
            Ok(Command::Quit)
        }
        _ => Err("unknown command; try HELP".to_string()),
    }
}

fn parse_stats<'a>(mut parts: impl Iterator<Item = &'a str>) -> Result<Command, String> {
    let kind = match parts.next().map(str::to_ascii_uppercase).as_deref() {
        None => StatsKind::Default,
        Some("CACHE") | Some("ITEMS") => StatsKind::Cache,
        Some("SLAB") | Some("SLABS") => StatsKind::Slab,
        Some("SIZES") => StatsKind::Sizes,
        Some(_) => return Err("usage: STATS [CACHE|SLAB|SIZES]".to_string()),
    };

    reject_extra_parts(parts, "usage: STATS [CACHE|SLAB|SIZES]")?;
    Ok(Command::Stats { kind })
}

fn parse_cachedump<'a>(mut parts: impl Iterator<Item = &'a str>) -> Result<Command, String> {
    let slab_id = required_part(&mut parts, "CACHEDUMP requires SLAB_ID")?
        .parse::<u32>()
        .map_err(|_| "CACHEDUMP SLAB_ID must be a non-negative integer".to_string())?;
    let limit = required_part(&mut parts, "CACHEDUMP requires LIMIT")?
        .parse::<u32>()
        .map_err(|_| "CACHEDUMP LIMIT must be a non-negative integer".to_string())?;

    reject_extra_parts(parts, "usage: CACHEDUMP SLAB_ID LIMIT")?;
    Ok(Command::CacheDump { slab_id, limit })
}

fn parse_storage_with_expiration<'a>(
    parts: impl Iterator<Item = &'a str>,
    command: &str,
) -> Result<Command, String> {
    let tokens: Vec<_> = parts.collect();
    if tokens.len() < 3 {
        return Err(format!("usage: {command} KEY VALUE EXPIRES_IN_SECONDS"));
    }

    let key = tokens[0];
    let expires_in_seconds = tokens[tokens.len() - 1]
        .parse::<u32>()
        .map_err(|_| format!("{command} EXPIRES_IN_SECONDS must be a non-negative integer"))?;
    let value = tokens[1..tokens.len() - 1].join(" ");

    validate_key(key)?;

    match command {
        "SET" => Ok(Command::Set {
            key: key.to_string(),
            value,
            expires_in_seconds,
        }),
        "ADD" => Ok(Command::Add {
            key: key.to_string(),
            value,
            expires_in_seconds,
        }),
        "REPLACE" => Ok(Command::Replace {
            key: key.to_string(),
            value,
            expires_in_seconds,
        }),
        _ => unreachable!("unknown storage command"),
    }
}

fn parse_storage_value<'a>(
    mut parts: impl Iterator<Item = &'a str>,
    command: &str,
) -> Result<Command, String> {
    let key = required_part(&mut parts, &format!("{command} requires KEY"))?;
    let value = parts.collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        return Err(format!("usage: {command} KEY VALUE"));
    }

    validate_key(key)?;

    match command {
        "APPEND" => Ok(Command::Append {
            key: key.to_string(),
            value,
        }),
        "PREPEND" => Ok(Command::Prepend {
            key: key.to_string(),
            value,
        }),
        _ => unreachable!("unknown storage command"),
    }
}

fn parse_cas<'a>(parts: impl Iterator<Item = &'a str>) -> Result<Command, String> {
    let tokens: Vec<_> = parts.collect();
    if tokens.len() < 4 {
        return Err("usage: CAS KEY VALUE EXPIRES_IN_SECONDS CAS_ID".to_string());
    }

    let key = tokens[0];
    let expires_in_seconds = tokens[tokens.len() - 2]
        .parse::<u32>()
        .map_err(|_| "CAS EXPIRES_IN_SECONDS must be a non-negative integer".to_string())?;
    let cas_id = tokens[tokens.len() - 1]
        .parse::<u64>()
        .map_err(|_| "CAS CAS_ID must be a non-negative integer".to_string())?;
    let value = tokens[1..tokens.len() - 2].join(" ");

    validate_key(key)?;

    Ok(Command::Cas {
        key: key.to_string(),
        value,
        expires_in_seconds,
        cas_id,
    })
}

fn parse_counter<'a>(
    mut parts: impl Iterator<Item = &'a str>,
    command: &str,
) -> Result<Command, String> {
    let key = required_part(&mut parts, &format!("{command} requires KEY"))?;
    let amount = required_part(&mut parts, &format!("{command} requires AMOUNT"))?
        .parse::<u64>()
        .map_err(|_| format!("{command} AMOUNT must be a non-negative integer"))?;

    reject_extra_parts(parts, &format!("usage: {command} KEY AMOUNT"))?;
    validate_key(key)?;

    match command {
        "INCR" => Ok(Command::Incr {
            key: key.to_string(),
            amount,
        }),
        "DECR" => Ok(Command::Decr {
            key: key.to_string(),
            amount,
        }),
        _ => unreachable!("unknown counter command"),
    }
}

fn parse_touch<'a>(mut parts: impl Iterator<Item = &'a str>) -> Result<Command, String> {
    let key = required_part(&mut parts, "TOUCH requires KEY")?;
    let expires_in_seconds = required_part(&mut parts, "TOUCH requires EXPIRES_IN_SECONDS")?
        .parse::<u32>()
        .map_err(|_| "TOUCH EXPIRES_IN_SECONDS must be a non-negative integer".to_string())?;

    reject_extra_parts(parts, "usage: TOUCH KEY EXPIRES_IN_SECONDS")?;
    validate_key(key)?;

    Ok(Command::Touch {
        key: key.to_string(),
        expires_in_seconds,
    })
}

fn optional_expires_in_seconds<'a>(
    mut parts: impl Iterator<Item = &'a str>,
    usage: &str,
) -> Result<Option<u32>, String> {
    let Some(expires_in_seconds) = parts.next() else {
        return Ok(None);
    };

    reject_extra_parts(parts, usage)?;
    expires_in_seconds
        .parse::<u32>()
        .map(Some)
        .map_err(|_| "EXPIRES_IN_SECONDS must be a non-negative integer".to_string())
}

fn required_part<'a>(
    parts: &mut impl Iterator<Item = &'a str>,
    message: &str,
) -> Result<&'a str, String> {
    parts.next().ok_or_else(|| message.to_string())
}

fn reject_extra_parts<'a>(
    mut parts: impl Iterator<Item = &'a str>,
    message: &str,
) -> Result<(), String> {
    if parts.next().is_some() {
        Err(message.to_string())
    } else {
        Ok(())
    }
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("key cannot be empty".to_string());
    }

    if key.len() > 250 {
        return Err("key must be 250 bytes or fewer".to_string());
    }

    if key.bytes().any(|byte| byte.is_ascii_control()) {
        return Err("key cannot contain control characters".to_string());
    }

    Ok(())
}

fn parse_value_header(line: &str) -> io::Result<ValueHeader> {
    let mut parts = line.split_whitespace();
    if parts.next() != Some("VALUE") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid value response",
        ));
    }

    let key = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing value key"))?
        .to_string();
    let _flags = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing value flags"))?;
    let byte_count = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing value byte count"))?
        .parse::<usize>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid value byte count"))?;
    let cas_id = parts
        .next()
        .map(|cas_id| {
            cas_id
                .parse::<u64>()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid CAS id"))
        })
        .transpose()?;

    Ok(ValueHeader {
        key,
        byte_count,
        cas_id,
    })
}

fn print_help() {
    println!("commands:");
    println!("  GET KEY");
    println!("  GETS KEY");
    println!("    Like GET, but includes the CAS id for use with CAS");
    println!("  SET KEY VALUE EXPIRES_IN_SECONDS");
    println!("  ADD KEY VALUE EXPIRES_IN_SECONDS");
    println!("  REPLACE KEY VALUE EXPIRES_IN_SECONDS");
    println!("  CAS KEY VALUE EXPIRES_IN_SECONDS CAS_ID");
    println!("    EXPIRES_IN_SECONDS is how long to keep the value; use 0 to never expire");
    println!("  APPEND KEY VALUE");
    println!("  PREPEND KEY VALUE");
    println!("  DELETE KEY");
    println!("  INCR KEY AMOUNT");
    println!("  DECR KEY AMOUNT");
    println!("  TOUCH KEY EXPIRES_IN_SECONDS");
    println!("  FLUSHALL [EXPIRES_IN_SECONDS]");
    println!("    Expires all keys now, or after EXPIRES_IN_SECONDS if provided");
    println!("  CACHEDUMP SLAB_ID LIMIT");
    println!("    Debug command for stats cachedump; use STATS CACHE to find slab ids");
    println!("  STATS");
    println!("  STATS CACHE");
    println!("    Shows item/cache stats via memcache's stats items command");
    println!("  STATS SLAB");
    println!("    Shows slab allocator stats via memcache's stats slabs command");
    println!("  STATS SIZES");
    println!("    Can be expensive on large servers; use cautiously");
    println!("  VERSION");
    println!("  HELP");
    println!("  QUIT");
}

fn print_response_lines(lines: &[String]) {
    if lines.is_empty() {
        println!("(empty)");
        return;
    }

    if page_response_lines(lines).unwrap_or(false) {
        return;
    }

    for line in lines {
        println!("{line}");
    }
}

fn page_response_lines(lines: &[String]) -> io::Result<bool> {
    if lines.len() <= pager_line_threshold() || !io::stdout().is_terminal() {
        return Ok(false);
    }

    let pager = env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    let Some((program, args)) = pager_command(&pager) else {
        return Ok(false);
    };

    let mut child = ProcessCommand::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()?;

    let mut write_error = None;
    if let Some(stdin) = child.stdin.as_mut() {
        for line in lines {
            if let Err(err) = writeln!(stdin, "{line}") {
                if err.kind() == io::ErrorKind::BrokenPipe {
                    break;
                }

                write_error = Some(err);
                break;
            }
        }
    }

    drop(child.stdin.take());
    let status = child.wait()?;
    if let Some(err) = write_error {
        return Err(err);
    }

    Ok(status.success())
}

fn pager_line_threshold() -> usize {
    env::var("LINES")
        .ok()
        .and_then(|lines| lines.parse::<usize>().ok())
        .filter(|lines| *lines > 1)
        .map(|lines| lines - 1)
        .unwrap_or(DEFAULT_TERMINAL_ROWS - 1)
}

fn pager_command(pager: &str) -> Option<(&str, Vec<&str>)> {
    let mut parts = pager.split_whitespace();
    let program = parts.next()?;
    Some((program, parts.collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_defaults() {
        assert_eq!(
            parse_cli_args(std::iter::empty::<&str>()),
            Ok(CliAction::Run(CliConfig {
                host: DEFAULT_HOST.to_string(),
                port: DEFAULT_PORT,
            }))
        );
    }

    #[test]
    fn parses_cli_host_and_port() {
        assert_eq!(
            parse_cli_args(["--host", "cache.example.com", "--port", "11212"]),
            Ok(CliAction::Run(CliConfig {
                host: "cache.example.com".to_string(),
                port: 11212,
            }))
        );
        assert_eq!(
            parse_cli_args(["--host=localhost", "--port=11213"]),
            Ok(CliAction::Run(CliConfig {
                host: "localhost".to_string(),
                port: 11213,
            }))
        );
    }

    #[test]
    fn parses_cli_help() {
        assert_eq!(parse_cli_args(["--help"]), Ok(CliAction::Help));
    }

    #[test]
    fn rejects_bad_cli_args() {
        assert_eq!(
            parse_cli_args(["--port", "nope"]),
            Err("--port must be a number between 1 and 65535".to_string())
        );
        assert_eq!(
            parse_cli_args(["--host"]),
            Err("--host requires a value".to_string())
        );
        assert_eq!(
            parse_cli_args(["127.0.0.1:11211"]),
            Err(format!(
                "unexpected argument: 127.0.0.1:11211\n{}",
                cli_usage()
            ))
        );
    }

    #[test]
    fn completes_repl_commands() {
        let (start, completions) = repl_command_completions("ge", 2);
        let replacements = completions
            .iter()
            .map(|completion| completion.replacement.as_str())
            .collect::<Vec<_>>();
        assert_eq!(start, 0);
        assert_eq!(replacements, vec!["GET ", "GETS "]);

        let (start, completions) = repl_command_completions("  del", 5);
        assert_eq!(start, 2);
        assert_eq!(completions[0].replacement, "DELETE ");
    }

    #[test]
    fn completes_exact_repl_command_with_trailing_space() {
        let (start, completions) = repl_command_completions("GET", 3);
        let replacements = completions
            .iter()
            .map(|completion| completion.replacement.as_str())
            .collect::<Vec<_>>();

        assert_eq!(start, 0);
        assert_eq!(replacements, vec!["GET "]);
    }

    #[test]
    fn does_not_complete_repl_command_arguments() {
        let (_start, completions) = repl_command_completions("GET my", 6);
        assert!(completions.is_empty());
    }

    #[test]
    fn parses_get() {
        assert_eq!(
            parse_command("GET my-key"),
            Ok(Command::Get {
                key: "my-key".to_string()
            })
        );
    }

    #[test]
    fn parses_set_with_spaces_in_value() {
        assert_eq!(
            parse_command("SET greeting hello world 30"),
            Ok(Command::Set {
                key: "greeting".to_string(),
                value: "hello world".to_string(),
                expires_in_seconds: 30,
            })
        );
    }

    #[test]
    fn parses_storage_commands() {
        assert_eq!(
            parse_command("ADD greeting hello 30"),
            Ok(Command::Add {
                key: "greeting".to_string(),
                value: "hello".to_string(),
                expires_in_seconds: 30,
            })
        );
        assert_eq!(
            parse_command("REPLACE greeting hello 30"),
            Ok(Command::Replace {
                key: "greeting".to_string(),
                value: "hello".to_string(),
                expires_in_seconds: 30,
            })
        );
        assert_eq!(
            parse_command("APPEND greeting there friend"),
            Ok(Command::Append {
                key: "greeting".to_string(),
                value: "there friend".to_string(),
            })
        );
        assert_eq!(
            parse_command("PREPEND greeting well hello"),
            Ok(Command::Prepend {
                key: "greeting".to_string(),
                value: "well hello".to_string(),
            })
        );
    }

    #[test]
    fn parses_gets_and_cas() {
        assert_eq!(
            parse_command("GETS greeting"),
            Ok(Command::Gets {
                key: "greeting".to_string()
            })
        );
        assert_eq!(
            parse_command("CAS greeting hello world 30 123"),
            Ok(Command::Cas {
                key: "greeting".to_string(),
                value: "hello world".to_string(),
                expires_in_seconds: 30,
                cas_id: 123,
            })
        );
    }

    #[test]
    fn parses_delete() {
        assert_eq!(
            parse_command("delete my-key"),
            Ok(Command::Delete {
                key: "my-key".to_string()
            })
        );
    }

    #[test]
    fn parses_flush_all() {
        assert_eq!(
            parse_command("FLUSHALL"),
            Ok(Command::FlushAll {
                expires_in_seconds: None
            })
        );
        assert_eq!(
            parse_command("FLUSHALL 10"),
            Ok(Command::FlushAll {
                expires_in_seconds: Some(10)
            })
        );
    }

    #[test]
    fn parses_cachedump() {
        assert_eq!(
            parse_command("CACHEDUMP 3 100"),
            Ok(Command::CacheDump {
                slab_id: 3,
                limit: 100,
            })
        );
        assert_eq!(
            parse_command("CACHE_DUMP 4 25"),
            Ok(Command::CacheDump {
                slab_id: 4,
                limit: 25,
            })
        );
    }

    #[test]
    fn parses_counter_touch_and_version() {
        assert_eq!(
            parse_command("INCR counter 2"),
            Ok(Command::Incr {
                key: "counter".to_string(),
                amount: 2,
            })
        );
        assert_eq!(
            parse_command("DECR counter 1"),
            Ok(Command::Decr {
                key: "counter".to_string(),
                amount: 1,
            })
        );
        assert_eq!(
            parse_command("TOUCH greeting 60"),
            Ok(Command::Touch {
                key: "greeting".to_string(),
                expires_in_seconds: 60,
            })
        );
        assert_eq!(parse_command("VERSION"), Ok(Command::Version));
    }

    #[test]
    fn parses_stats_commands() {
        assert_eq!(
            parse_command("STATS"),
            Ok(Command::Stats {
                kind: StatsKind::Default
            })
        );
        assert_eq!(
            parse_command("STATS CACHE"),
            Ok(Command::Stats {
                kind: StatsKind::Cache
            })
        );
        assert_eq!(
            parse_command("STATS SLAB"),
            Ok(Command::Stats {
                kind: StatsKind::Slab
            })
        );
        assert_eq!(
            parse_command("STATS SIZES"),
            Ok(Command::Stats {
                kind: StatsKind::Sizes
            })
        );
    }

    #[test]
    fn rejects_bad_set_ttl() {
        assert_eq!(
            parse_command("SET greeting hello nope"),
            Err("SET EXPIRES_IN_SECONDS must be a non-negative integer".to_string())
        );
    }

    #[test]
    fn rejects_unknown_stats_kind() {
        assert_eq!(
            parse_command("STATS nope"),
            Err("usage: STATS [CACHE|SLAB|SIZES]".to_string())
        );
    }

    #[test]
    fn rejects_bad_cachedump() {
        assert_eq!(
            parse_command("CACHEDUMP nope 100"),
            Err("CACHEDUMP SLAB_ID must be a non-negative integer".to_string())
        );
        assert_eq!(
            parse_command("CACHEDUMP 3 nope"),
            Err("CACHEDUMP LIMIT must be a non-negative integer".to_string())
        );
    }

    #[test]
    fn splits_pager_command() {
        assert_eq!(pager_command("less -R"), Some(("less", vec!["-R"])));
        assert_eq!(pager_command(""), None);
    }
}
