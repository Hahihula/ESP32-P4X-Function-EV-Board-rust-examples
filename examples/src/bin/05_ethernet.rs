//! Demo 05: Ethernet.
//!
//! Brings up the Ethernet port and `embassy-net`, gets an address by DHCP,
//! sets the clock from `pool.ntp.org` (SNTP), then serves a status page over
//! HTTP on port 80. The board also answers ping.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use embassy_executor::Spawner;
use embassy_net::{
    Config, IpAddress, Runner, Stack, StackResources,
    dns::DnsQueryType,
    tcp::TcpSocket,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_time::{Duration, Instant, with_timeout};
use esp_backtrace as _;
use esp_hal::rng::Rng;
use esp32_p4_function_ev::{
    ethernet::{self, DmaStorage, EthernetPort},
    ntp::{self, DateTime},
};
use log::{info, warn};
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

const NTP_SERVER: &str = "pool.ntp.org";
const NTP_TIMEOUT: Duration = Duration::from_secs(5);
const HTTP_PORT: u16 = 80;
/// Give up on a client that sends nothing for this long.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// Sockets: DHCP, DNS, the SNTP UDP socket and the HTTP TCP socket.
const SOCKETS: usize = 4;

static DMA_STORAGE: StaticCell<DmaStorage> = StaticCell::new();
static STACK_RESOURCES: StaticCell<StackResources<SOCKETS>> = StaticCell::new();

/// Wall-clock time: the Unix time from SNTP and when it was received.
#[derive(Clone, Copy)]
struct Clock {
    unix: u64,
    at: Instant,
}

impl Clock {
    fn now(&self) -> DateTime {
        DateTime::from_unix(self.unix + self.at.elapsed().as_secs())
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp32_p4_function_ev_examples::start!(p, "Demo 05: Ethernet");

    let port = EthernetPort::new(
        p.ETH,
        esp32_p4_function_ev::eth_pins!(p),
        DMA_STORAGE.init(DmaStorage::new()),
    )
    .await
    .unwrap();
    info!("MAC address {}", MacDisplay(ethernet::mac_address()));

    let rng = Rng::new();
    let seed = u64::from(rng.random()) << 32 | u64::from(rng.random());
    let (stack, runner) = embassy_net::new(
        port,
        Config::dhcpv4(Default::default()),
        STACK_RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(net_task(runner).unwrap());

    info!("Waiting for the link (is the cable plugged in?)");
    stack.wait_link_up().await;
    info!("Link up, asking for an address by DHCP");
    stack.wait_config_up().await;
    let Some(config) = stack.config_v4() else {
        panic!("DHCP finished without an IPv4 address");
    };
    info!(
        "IP address {}, gateway {:?}, DNS {:?}",
        config.address, config.gateway, config.dns_servers
    );

    let clock = match sntp(stack).await {
        Ok(clock) => {
            info!("Time from {NTP_SERVER}: {}", clock.now());
            Some(clock)
        }
        Err(e) => {
            warn!("SNTP failed: {e}");
            None
        }
    };

    info!(
        "Open http://{}/ in a browser, or ping it",
        config.address.address()
    );
    serve_http(stack, clock).await
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, EthernetPort<'static>>) -> ! {
    runner.run().await
}

/// Asks the NTP pool for the time.
#[allow(
    clippy::large_stack_frames,
    reason = "the UDP socket buffers live here; awaited once from main"
)]
async fn sntp(stack: Stack<'_>) -> Result<Clock, &'static str> {
    let addresses = stack
        .dns_query(NTP_SERVER, DnsQueryType::A)
        .await
        .map_err(|_| "DNS lookup failed")?;
    let server: IpAddress = *addresses.first().ok_or("no DNS answer")?;

    let mut rx_meta = [PacketMetadata::EMPTY; 1];
    let mut tx_meta = [PacketMetadata::EMPTY; 1];
    let mut rx_buf = [0u8; 128];
    let mut tx_buf = [0u8; 128];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    socket.bind(0).map_err(|_| "UDP bind failed")?;
    socket
        .send_to(&ntp::request(), (server, ntp::PORT))
        .await
        .map_err(|_| "UDP send failed")?;

    let mut reply = [0u8; ntp::PACKET_LEN];
    let (len, _) = with_timeout(NTP_TIMEOUT, socket.recv_from(&mut reply))
        .await
        .map_err(|_| "no reply")?
        .map_err(|_| "UDP receive failed")?;
    let unix = ntp::parse_response(&reply[..len]).ok_or("bad reply")?;
    Ok(Clock {
        unix,
        at: Instant::now(),
    })
}

/// Serves one HTTP client at a time, forever.
#[allow(
    clippy::large_stack_frames,
    reason = "the socket buffers live here; awaited once from main"
)]
async fn serve_http(stack: Stack<'_>, clock: Option<Clock>) -> ! {
    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 2048];
    let mut served = 0u32;
    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(HTTP_TIMEOUT));
        if let Err(e) = socket.accept(HTTP_PORT).await {
            warn!("accept: {e:?}");
            continue;
        }
        let peer = socket.remote_endpoint();

        // Read the request head; only its first line matters here.
        let mut request = [0u8; 512];
        let mut len = 0;
        while len < request.len() && !request[..len].windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut request[len..]).await {
                Ok(0) | Err(_) => break,
                Ok(n) => len += n,
            }
        }
        if len == 0 {
            // Browsers open spare connections and may close them unused.
            socket.abort();
            continue;
        }
        let first_line = core::str::from_utf8(&request[..len])
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("");
        let path = first_line.split(' ').nth(1).unwrap_or("");

        let response = if first_line.starts_with("GET ") && path == "/" {
            served += 1;
            let body = status_page(stack, clock, served);
            alloc::format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        } else {
            alloc::string::String::from(
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
        };
        info!(
            "{first_line:?} from {peer:?}: {}",
            &response[9..response.find('\r').unwrap_or(9)]
        );
        if let Err(e) = write_all(&mut socket, response.as_bytes()).await {
            warn!("write: {e:?}");
        }
        // `flush` waits until the client has acknowledged all data, so the
        // connection can then be reset rather than closed gracefully, and
        // the socket is free for the next client straight away.
        let _ = socket.flush().await;
        socket.abort();
        let _ = socket.flush().await;
    }
}

async fn write_all(
    socket: &mut TcpSocket<'_>,
    mut data: &[u8],
) -> Result<(), embassy_net::tcp::Error> {
    while !data.is_empty() {
        let n = socket.write(data).await?;
        data = &data[n..];
    }
    Ok(())
}

fn status_page(stack: Stack<'_>, clock: Option<Clock>, served: u32) -> alloc::string::String {
    let uptime = Instant::now().as_secs();
    let ip = stack
        .config_v4()
        .map(|c| alloc::format!("{}", c.address))
        .unwrap_or_default();
    let time = clock.map_or_else(
        || alloc::string::String::from("unknown (SNTP failed)"),
        |c| alloc::format!("{}", c.now()),
    );
    alloc::format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"5\">\
         <title>ESP32-P4</title>\
         <style>body{{font-family:sans-serif;margin:2em}}td{{padding:.2em 1em}}</style>\
         </head><body><h1>ESP32-P4X-Function-EV-Board</h1>\
         <p>Served by Rust, esp-hal and embassy-net. Refreshes every 5 s.</p><table>\
         <tr><td>Time</td><td>{time}</td></tr>\
         <tr><td>Uptime</td><td>{}:{:02}:{:02}</td></tr>\
         <tr><td>IP address</td><td>{ip}</td></tr>\
         <tr><td>MAC address</td><td>{}</td></tr>\
         <tr><td>Pages served</td><td>{served}</td></tr>\
         </table></body></html>",
        uptime / 3600,
        uptime / 60 % 60,
        uptime % 60,
        MacDisplay(ethernet::mac_address()),
    )
}

/// Formats a MAC address as `aa:bb:cc:dd:ee:ff`.
struct MacDisplay([u8; 6]);

impl core::fmt::Display for MacDisplay {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}
