use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use hickory_proto::{
    op::{Message, MessageType, OpCode, Query, ResponseCode},
    rr::{Name, RData, RecordType},
};
use native_tls::TlsConnector;
use serde_json::Value;

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const SESSION_TIMEOUT: Duration = Duration::from_secs(120);
const LIMIT: usize = 64;

#[derive(Clone)]
pub struct Config {
    endpoint: SocketAddr,
    name: String,
    family: &'static str,
    require_ad: bool,
}

impl Config {
    pub fn settings(&self) -> Value {
        serde_json::json!({"mode": "tls", "address": self.endpoint.to_string(), "server_name": self.name, "family": self.family, "require_ad": self.require_ad})
    }

    pub fn parse(value: &Value) -> Result<Option<Self>, &'static str> {
        if value.is_null() {
            return Ok(None);
        }
        if !value.is_object() {
            return Err("DNS configuration must be an object.");
        }
        if value.get("mode").is_some_and(|mode| !mode.is_string()) {
            return Err("Unknown DNS mode.");
        }
        match value["mode"].as_str().unwrap_or("upstream") {
            "upstream" => return Ok(None),
            "tls" => (),
            _ => return Err("Unknown DNS mode."),
        }
        let endpoint = value["address"]
            .as_str()
            .unwrap_or("1.1.1.1:853")
            .parse::<SocketAddr>()
            .ok()
            .filter(|addr| addr.port() != 0)
            .ok_or("Use a numeric DNS IP and port.")?;
        let name = value["server_name"]
            .as_str()
            .unwrap_or("cloudflare-dns.com")
            .trim()
            .to_owned();
        if !name.contains('.')
            || name.parse::<IpAddr>().is_ok()
            || !name.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
            || name.len() > 253
        {
            return Err("Use a DNS hostname for certificate verification.");
        }
        let family = match value["family"].as_str().unwrap_or("dual") {
            "dual" => "dual",
            "ipv4" => "ipv4",
            "ipv6" => "ipv6",
            _ => return Err("Unknown DNS address family."),
        };
        Ok(Some(Self {
            endpoint,
            name,
            family,
            require_ad: value["require_ad"].as_bool().unwrap_or(false),
        }))
    }
}

pub struct Relay {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl Relay {
    pub fn start(upstream: SocketAddr, config: Config) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let tls = TlsConnector::builder()
            .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
            .build()
            .map_err(io::Error::other)?;
        let backend = Arc::new(Backend {
            upstream,
            config,
            tls,
            cache: Mutex::new(HashMap::new()),
        });
        let engine_stop = stop.clone();
        let count = Arc::new(AtomicUsize::new(0));
        thread::Builder::new()
            .name("private-dns-relay".into())
            .spawn(move || {
                for accepted in listener.incoming() {
                    if engine_stop.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(client) = accepted else { break };
                    if count.fetch_add(1, Ordering::AcqRel) >= LIMIT {
                        count.fetch_sub(1, Ordering::AcqRel);
                        continue;
                    }
                    let count = count.clone();
                    let backend = backend.clone();
                    if thread::Builder::new()
                        .spawn({
                            let count = count.clone();
                            move || {
                                let _ = backend.serve(client);
                                count.fetch_sub(1, Ordering::AcqRel);
                            }
                        })
                        .is_err()
                    {
                        count.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            })?;
        Ok(Self { address, stop })
    }

    pub fn proxy_url(&self) -> String {
        format!("socks5://{}", self.address)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(100));
    }
}

type Cache = HashMap<String, (Instant, Vec<IpAddr>)>;

struct Backend {
    upstream: SocketAddr,
    config: Config,
    tls: TlsConnector,
    cache: Mutex<Cache>,
}

enum Target {
    Ip(IpAddr),
    Host(String),
}

impl Backend {
    fn serve(&self, mut client: TcpStream) -> io::Result<()> {
        timeouts(&client, IO_TIMEOUT)?;
        let [version, count] = bytes(&mut client)?;
        let mut methods = vec![0; usize::from(count)];
        client.read_exact(&mut methods)?;
        if version != 5 || !methods.contains(&0) {
            client.write_all(&[5, 255])?;
            return Ok(());
        }
        client.write_all(&[5, 0])?;
        let [version, command, reserved, format] = bytes(&mut client)?;
        if (version, command, reserved) != (5, 1, 0) {
            return reply(&mut client, 7);
        }
        let target = match format {
            1 => Target::Ip(Ipv4Addr::from(bytes::<4>(&mut client)?).into()),
            3 => {
                let [len] = bytes(&mut client)?;
                let mut name = vec![0; usize::from(len)];
                client.read_exact(&mut name)?;
                Target::Host(String::from_utf8(name).map_err(io::Error::other)?)
            }
            4 => Target::Ip(Ipv6Addr::from(bytes::<16>(&mut client)?).into()),
            _ => return reply(&mut client, 8),
        };
        let port = u16::from_be_bytes(bytes(&mut client)?);
        let candidates = match target {
            Target::Ip(ip) => vec![ip],
            Target::Host(host) => match self.resolve(&host) {
                Ok(addresses) => addresses,
                Err(_) => return reply(&mut client, 4),
            },
        };
        for ip in candidates.into_iter().take(8) {
            if let Ok(remote) = self.connect(SocketAddr::new(ip, port)) {
                timeouts(&client, SESSION_TIMEOUT)?;
                timeouts(&remote, SESSION_TIMEOUT)?;
                reply(&mut client, 0)?;
                return pipe(client, remote);
            }
        }
        reply(&mut client, 4)
    }

    fn connect(&self, target: SocketAddr) -> io::Result<TcpStream> {
        let mut stream = TcpStream::connect_timeout(&self.upstream, IO_TIMEOUT)?;
        timeouts(&stream, IO_TIMEOUT)?;
        stream.write_all(&[5, 1, 0])?;
        if bytes::<2>(&mut stream)? != [5, 0] {
            return Err(failure("upstream SOCKS authentication failed"));
        }
        let mut connect = vec![5, 1, 0];
        match target.ip() {
            IpAddr::V4(ip) => {
                connect.push(1);
                connect.extend(ip.octets());
            }
            IpAddr::V6(ip) => {
                connect.push(4);
                connect.extend(ip.octets());
            }
        }
        connect.extend(target.port().to_be_bytes());
        stream.write_all(&connect)?;
        let [version, status, reserved, address_type] = bytes(&mut stream)?;
        if (version, status, reserved) != (5, 0, 0) {
            return Err(failure("upstream SOCKS connection failed"));
        }
        let length = match address_type {
            1 => 4,
            3 => usize::from(bytes::<1>(&mut stream)?[0]),
            4 => 16,
            _ => return Err(failure("invalid SOCKS address type")),
        };
        let mut ignored = vec![0; length + 2];
        stream.read_exact(&mut ignored)?;
        Ok(stream)
    }

    fn resolve(&self, domain: &str) -> io::Result<Vec<IpAddr>> {
        let name = Name::from_ascii(domain).map_err(io::Error::other)?;
        let mut name = name.to_lowercase();
        name.set_fqdn(true);
        let key = name.to_ascii();
        if let Some((expiry, ips)) = self
            .cache
            .lock()
            .map_err(|_| failure("DNS cache unavailable"))?
            .get(&key)
        {
            if *expiry > Instant::now() {
                return Ok(ips.clone());
            }
        }
        let tcp = self.connect(self.config.endpoint)?;
        let mut stream = self
            .tls
            .connect(&self.config.name, tcp)
            .map_err(io::Error::other)?;
        let types: &[RecordType] = match self.config.family {
            "ipv4" => &[RecordType::A],
            "ipv6" => &[RecordType::AAAA],
            _ => &[RecordType::A, RecordType::AAAA],
        };
        let mut ips = Vec::new();
        let mut ttl = 300;
        for &kind in types {
            let (addresses, answer_ttl) = lookup(&mut stream, &name, kind, self.config.require_ad)?;
            ips.extend(addresses);
            ttl = ttl.min(answer_ttl);
        }
        if ips.is_empty() {
            return Err(failure("empty DNS response"));
        }
        if ttl > 0 {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| failure("DNS cache unavailable"))?;
            if cache.len() >= 512 {
                let now = Instant::now();
                cache.retain(|_, (expiry, _)| *expiry > now);
                if cache.len() >= 512 {
                    cache.clear();
                }
            }
            cache.insert(
                key,
                (
                    Instant::now() + Duration::from_secs(u64::from(ttl)),
                    ips.clone(),
                ),
            );
        }
        Ok(ips)
    }
}

fn lookup(
    stream: &mut (impl Read + Write),
    name: &Name,
    kind: RecordType,
    require_ad: bool,
) -> io::Result<(Vec<IpAddr>, u32)> {
    let mut query = Message::query();
    query.metadata.recursion_desired = true;
    query.metadata.authentic_data = require_ad;
    query.add_query(Query::query(name.clone(), kind));
    let encoded = query.to_vec().map_err(io::Error::other)?;
    let length = u16::try_from(encoded.len()).map_err(io::Error::other)?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&encoded)?;
    stream.flush()?;
    let mut raw = vec![0; usize::from(u16::from_be_bytes(bytes(stream)?))];
    stream.read_exact(&mut raw)?;
    let answer = Message::from_vec(&raw).map_err(io::Error::other)?;
    if answer.metadata.id != query.metadata.id
        || answer.metadata.message_type != MessageType::Response
        || answer.metadata.op_code != OpCode::Query
        || answer.metadata.truncation
        || answer.queries != query.queries
        || answer.metadata.response_code != ResponseCode::NoError
        || (require_ad && !answer.metadata.authentic_data)
    {
        return Err(failure("DNS response rejected"));
    }
    let mut canonical = name;
    let mut ttl = 300;
    for _ in 0..16 {
        let alias = answer
            .answers
            .iter()
            .find(|record| &record.name == canonical && matches!(&record.data, RData::CNAME(_)));
        if let Some(record) = alias {
            let RData::CNAME(alias) = &record.data else {
                unreachable!()
            };
            canonical = &alias.0;
            ttl = ttl.min(record.ttl);
        } else {
            let ips = answer
                .answers
                .iter()
                .filter(|record| &record.name == canonical)
                .filter_map(|record| {
                    let ip = match (kind, &record.data) {
                        (RecordType::A, RData::A(ip)) => IpAddr::V4(ip.0),
                        (RecordType::AAAA, RData::AAAA(ip)) => IpAddr::V6(ip.0),
                        _ => return None,
                    };
                    ttl = ttl.min(record.ttl);
                    Some(ip)
                })
                .collect();
            return Ok((ips, ttl));
        }
    }
    Err(failure("DNS alias chain exceeds limit"))
}

fn reply(client: &mut TcpStream, status: u8) -> io::Result<()> {
    client.write_all(&[5, status, 0, 1, 0, 0, 0, 0, 0, 0])
}

fn timeouts(stream: &TcpStream, timeout: Duration) -> io::Result<()> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))
}

fn pipe(mut client: TcpStream, mut remote: TcpStream) -> io::Result<()> {
    let mut inbound = client.try_clone()?;
    let mut outbound = remote.try_clone()?;
    thread::scope(|scope| {
        let forward = scope.spawn(move || {
            let _ = io::copy(&mut inbound, &mut outbound);
            let _ = outbound.shutdown(Shutdown::Write);
        });
        let result = io::copy(&mut remote, &mut client);
        let _ = client.shutdown(Shutdown::Both);
        let _ = remote.shutdown(Shutdown::Both);
        let _ = forward.join();
        result.map(|_| ())
    })
}

fn bytes<const N: usize>(stream: &mut impl Read) -> io::Result<[u8; N]> {
    let mut buffer = [0; N];
    stream.read_exact(&mut buffer)?;
    Ok(buffer)
}

fn failure(message: &'static str) -> io::Error {
    io::Error::other(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::rr::{Record, rdata::A};

    #[test]
    #[ignore = "requires a local TLS test certificate"]
    fn resolves_over_authenticated_tls_inside_socks() {
        let fixture = std::path::PathBuf::from(std::env::var_os("ORBITAL_DNS_TEST_CERT").unwrap());
        let certificate = native_tls::Certificate::from_pem(
            &std::fs::read(fixture.join("resolver.crt")).unwrap(),
        )
        .unwrap();
        let identity = native_tls::Identity::from_pkcs12(
            &std::fs::read(fixture.join("resolver.pfx")).unwrap(),
            "local-test",
        )
        .unwrap();
        let acceptor = native_tls::TlsAcceptor::new(identity).unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let upstream = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            timeouts(&socket, IO_TIMEOUT).unwrap();
            assert_eq!(bytes::<3>(&mut socket).unwrap(), [5, 1, 0]);
            socket.write_all(&[5, 0]).unwrap();
            assert_eq!(
                bytes::<10>(&mut socket).unwrap(),
                [5, 1, 0, 1, 192, 0, 2, 53, 3, 85]
            );
            reply(&mut socket, 0).unwrap();
            let mut tls = acceptor.accept(socket).unwrap();
            let size = u16::from_be_bytes(bytes(&mut tls).unwrap());
            let mut wire = vec![0; usize::from(size)];
            tls.read_exact(&mut wire).unwrap();
            let request = Message::from_vec(&wire).unwrap();
            assert_eq!(request.queries[0].name().to_ascii(), "orbital.test.");
            let mut answer = Message::response(request.metadata.id, OpCode::Query);
            answer.metadata.authentic_data = true;
            answer
                .add_query(request.queries[0].clone())
                .add_answer(Record::from_rdata(
                    request.queries[0].name().clone(),
                    60,
                    RData::A(A(Ipv4Addr::new(192, 0, 2, 80))),
                ));
            let wire = answer.to_vec().unwrap();
            tls.write_all(&(wire.len() as u16).to_be_bytes()).unwrap();
            tls.write_all(&wire).unwrap();
            tls.flush().unwrap();
        });
        let tls = TlsConnector::builder()
            .add_root_certificate(certificate)
            .build()
            .unwrap();
        let config = Config {
            endpoint: "192.0.2.53:853".parse().unwrap(),
            name: "resolver.orbital.test".into(),
            family: "ipv4",
            require_ad: true,
        };
        let backend = Backend {
            upstream,
            config,
            tls,
            cache: Mutex::new(HashMap::new()),
        };
        let expected = vec![IpAddr::from([192, 0, 2, 80])];
        assert_eq!(backend.resolve("orbital.test").unwrap(), expected);
        server.join().unwrap();
        assert_eq!(backend.resolve("orbital.test").unwrap(), expected);
    }
}
