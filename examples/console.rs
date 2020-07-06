use std::convert::TryFrom;
use std::fmt;
use std::io::Write;
use std::net::TcpStream;

use aprs_parser::{self, APRSData, Timestamp};
use futures::io::{self, BufReader};
use futures::prelude::*;
use smol::Async;

const APP_NAME: &str = env!("CARGO_PKG_NAME");
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

// Port 14580 is filtered
const HOST: &str = "aprs.glidernet.org:14580";

fn timestamp_to_str(ts: Timestamp) -> String {
    match ts {
        Timestamp::DDHHMM(d, h, m) => format!("{:02}/{:02}:{:02}", d, h, m),
        Timestamp::HHMMSS(h, m, s) => format!("Today/{:02}:{:02}:{:02}", h, m, s),
        Timestamp::Unsupported(val) => val.to_string(),
    }
}

#[derive(Debug)]
struct OgnComment {
    address: String,
    stealth_mode: bool,
    no_tracking: bool,
    aircraft_type: AircraftType,
    address_type: AddressType,
}

/// Aircraft type.
///
/// See `AcftType` in FLARM DataPort Specification[1].
///
/// [1]: http://www.ediatec.ch/pdf/FLARM%20Data%20Port%20Specification%20v7.00.pdf
#[derive(Debug, PartialEq, Eq)]
enum AircraftType {
    /// Unknown type
    Unknown,
    /// Glider / motor glidre
    Glider,
    /// Tow / tug plane
    TowPlane,
    /// Helicopter / rotorcraft
    Helicopter,
    /// Skydiver
    Skydiver,
    /// Drop plane for skydivers
    DropPlane,
    /// Hang glider (hard)
    Hangglider,
    /// Paraglider (soft)
    Paraglider,
    /// Aircraft with reciprocating engine(s)
    PoweredAircraft,
    /// Aircraft with jet/turboprop engine(s)
    JetAircraft,
    /// Balloon
    Balloon,
    /// Airship
    Airship,
    /// Unmanned aerial vehicle (UAV)
    Uav,
    /// Static object
    Static,
}

impl TryFrom<u8> for AircraftType {
    type Error = ();
    fn try_from(val: u8) -> Result<Self, Self::Error> {
        match val {
            0x0 => Ok(AircraftType::Unknown),
            0x1 => Ok(AircraftType::Glider),
            0x2 => Ok(AircraftType::TowPlane),
            0x3 => Ok(AircraftType::Helicopter),
            0x4 => Ok(AircraftType::Skydiver),
            0x5 => Ok(AircraftType::DropPlane),
            0x6 => Ok(AircraftType::Hangglider),
            0x7 => Ok(AircraftType::Paraglider),
            0x8 => Ok(AircraftType::PoweredAircraft),
            0x9 => Ok(AircraftType::JetAircraft),
            0xa => Ok(AircraftType::Unknown),
            0xb => Ok(AircraftType::Balloon),
            0xc => Ok(AircraftType::Airship),
            0xd => Ok(AircraftType::Uav),
            0xe => Ok(AircraftType::Static),
            0xf => Ok(AircraftType::Unknown),
            _ => Err(()),
        }
    }
}

impl fmt::Display for AircraftType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AddressType {
    Random,
    Icao,
    Flarm,
    Ogn,
}

impl TryFrom<u8> for AddressType {
    type Error = ();
    fn try_from(val: u8) -> Result<Self, Self::Error> {
        match val {
            0x0 => Ok(AddressType::Random),
            0x1 => Ok(AddressType::Icao),
            0x2 => Ok(AddressType::Flarm),
            0x3 => Ok(AddressType::Ogn),
            _ => Err(()),
        }
    }
}

impl fmt::Display for AddressType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", match self {
            Self::Random => "Random",
            Self::Icao => "ICAO",
            Self::Flarm => "FLARM",
            Self::Ogn => "OGN",
        })
    }
}

fn parse_ogn_comment(comment: &str) -> Option<OgnComment> {
    let mut iter = comment.split(" ").skip_while(|x| !x.starts_with("id"));
    let id = iter.next()?;
    let flags = u8::from_str_radix(&id[2..4], 16).ok()?;
    Some(OgnComment {
        address: id[4..].to_string(),
        stealth_mode: (flags & 0b10000000) > 0,
        no_tracking: (flags & 0b01000000) > 0,
        aircraft_type: AircraftType::try_from((flags & 0b00111100) >> 2).unwrap(),
        address_type: AddressType::try_from(flags & 0b00000011).unwrap(),
    })
}

async fn read_messages(client: Async<TcpStream>) -> io::Result<()> {
    // Create async stdout writer
    let mut stdout = smol::writer(std::io::stdout());

    // Line reader
    let mut lines = BufReader::new(client).lines();

    // Handle every incoming line
    while let Some(line) = lines.next().await {
        let line = line?;
        if line.starts_with("#") {
            // Comment
            stdout.write_all(line.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
        } else {
            // APRS data
            match aprs_parser::parse(&line) {
                Ok(parsed) => match parsed.data {
                    APRSData::Position(pos) => {
                        println!("{}", &pos.comment);
                        let comment = match parse_ogn_comment(&pos.comment) {
                            Some(comment) => comment,
                            None => continue,
                        };
                        if comment.aircraft_type != AircraftType::Paraglider {
                            continue;
                        }
                        let log = format!(
                            "{}: {:.6}/{:.6} ({} {} {} from {} to {} via {:?})\n",
                            pos.timestamp
                                .map(timestamp_to_str)
                                .unwrap_or_else(|| "?".to_string()),
                            pos.latitude,
                            pos.longitude,
                            comment.aircraft_type,
                            comment.address_type,
                            comment.address,
                            parsed.from.call,
                            parsed.to.call,
                            parsed
                                .via
                                .iter()
                                .map(|cs| cs.call.clone())
                                .collect::<Vec<_>>(),
                        );
                        stdout.write_all(log.as_bytes()).await?;
                    }
                    APRSData::Unknown => {
                        stdout
                            .write_all(format!("Unknown data: {}\n", line).as_bytes())
                            .await?
                    }
                },
                Err(e) => stdout.write_all(format!("Err: {}\n", e).as_bytes()).await?,
            }
        }
        stdout.flush().await?;
    }

    Ok(())
}

fn main() -> io::Result<()> {
    smol::run(async {
        // Connect to the server
        let mut stream = Async::<TcpStream>::connect(HOST).await?;
        if let Err(e) = stream.get_mut().set_nodelay(true) {
            eprintln!("Warning: Could not set TCP_NODELAY on socket: {}", e);
        }
        println!("Connected to {}", stream.get_ref().peer_addr()?);

        // Login
        // TODO: UDP support? http://www.aprs-is.net/ClientUDP.aspx
        let user = "bolOGNese"; // TODO
        let pass = "-1"; // Password for receive-only clients according to APRS-IS docs
        let filter = "r/47.217/8.804/30"; // 30 km around the specified coordinates
        let auth_line = format!(
            "user {} pass {} vers {} {} filter {}\r\n",
            user, pass, APP_NAME, APP_VERSION, filter,
        );
        stream
            .write_with_mut(|s| s.write(auth_line.as_bytes()))
            .await?;

        // Process incoming stream
        read_messages(stream).await?;

        Ok(())
    })
}
