extern crate nix;
extern crate rand;
extern crate reqwest;

#[macro_use]
extern crate serde_derive;

extern crate serde;
extern crate serde_json;

use nix::unistd::{execv, fork, gethostname, setsid, ForkResult};
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};

use std::io::{self, BufRead, BufReader, Read};
use std::fs::File;
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::CString;
use std::vec::Vec;
use std::thread;
use std::hash::Hasher;
use std::time::{Duration, Instant};
use std::env;

#[derive(Deserialize, Serialize, Debug, Copy, Clone, PartialEq)]
enum TreeState {
    Child,
    Parent,
    Sibling,
    This,
}

#[derive(Deserialize, Serialize, Debug)]
enum Message {
    SuicideNote(WormSegment),
    NewSegment(WormSegment),
    WantData(String),
    GatheringCompleted,
}

#[derive(Deserialize, Serialize, Debug, PartialEq)]
struct WormSegment {
    relationship: TreeState,
    hostname: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct Worm {
    initial_hostname: String,
    current_hostname: String, // Modify after sending
    max_num_segments: usize,
    cur_num_segments: usize,                   // Modify before sending
    observation_data: HashMap<String, String>, // Modify with gossiping and after getting data
    current_segments: Vec<WormSegment>, // Modify before sending and after sending (change state)
    hosts_to_ovserve: Vec<String>,
    wormgate_port: u16,
}

impl WormSegment {
    /// Create a new WormSegment based on a state and host
    pub fn new(rel: TreeState, host: &str) -> WormSegment {
        WormSegment {
            relationship: rel,
            hostname: String::from(host),
        }
    }

    /// Convert WormSegment such that we can send it to another segment
    pub fn send_to(&self, target: &WormSegment) -> WormSegment {
        let new_rel = match target.relationship {
            TreeState::Child => match self.relationship {
                TreeState::Child => TreeState::Sibling,
                TreeState::This => TreeState::Parent,
                _ => self.relationship,
            },
            TreeState::Sibling => match self.relationship {
                TreeState::This => TreeState::Sibling,
                _ => self.relationship,
            },
            _ => self.relationship,
        };

        if target.hostname == self.hostname {
            WormSegment::new(TreeState::This, &self.hostname)
        } else {
            WormSegment::new(new_rel, &self.hostname)
        }
    }
}

impl Worm {
    /// Create a new worm with max number of segments and a list of hosts
    ///
    /// Should only be used the very first time a worm is created,
    /// and the rest should simply be sent.
    pub fn new(max_segments: usize, worm_port: u16, hosts: Vec<String>) -> Worm {
        let mut buf = vec![0; 50];
        let hostname = gethostname(&mut buf)
            .expect("Error getting hostname")
            .to_str()
            .expect("Error using hostname as str")
            .split(".")
            .next()
            .expect("No . in hostname");

        Worm {
            initial_hostname: String::from(hostname),
            current_hostname: String::from(hostname),
            max_num_segments: max_segments,
            cur_num_segments: 1,
            observation_data: HashMap::new(),
            current_segments: vec![WormSegment::new(TreeState::This, hostname)],
            hosts_to_ovserve: hosts,
            wormgate_port: worm_port,
        }
    }

    /// Get data from wormgate on current host
    pub fn get_data(&mut self) {
        let map: HashMap<String, String> = reqwest::get(&format!(
            "http://localhost:{}/observation_data",
            self.wormgate_port
        )).expect("Error requesting observation data")
            .json()
            .expect("Error parsing JSON");

        for (k, v) in &map {
            // Strip away port number from data
            let host = k.split(":").next().expect("No : in the hostname");
            self.observation_data
                .insert(host.to_string(), v.to_string());
        }
    }

    /// Determine if the worm should infect a new host
    pub fn should_infect(&self) -> bool {
        self.cur_num_segments < self.max_num_segments
    }

    /// Listen for gossip from other WormSegments
    /// Insert data into the state struct
    pub fn listen_for_gossip(&mut self) {
        // Set timeout to 5 seconds
        let timeout = Duration::from_secs(5);
        let now = Instant::now();
        let listener = TcpListener::bind(format!(
            "{}:{}",
            &self.current_hostname,
            get_listen_port(false)
        )).expect("Error binding to port when listening for gossip");
        listener
            .set_nonblocking(true)
            .expect("Unable to make listener nonblocking");
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    // Accept connections and perform actions based on the message type received
                    // Can either receive a message about a new segment or someone wants data from
                    // the specific host
                    println!("We got a message!");
                    if let Ok(message) = serde_json::from_reader(&stream) {
                        let message: Message = message;
                        match message {
                            Message::NewSegment(segment) => {
                                println!("Got message regarding a new segment: {:?}", segment);
                                if !self.current_segments.contains(&segment) {
                                    self.current_segments.push(segment);
                                    self.cur_num_segments += 1;
                                }
                            }
                            Message::WantData(hostname) => {
                                stream
                                    .shutdown(Shutdown::Read)
                                    .expect("Read Shutdown failed");
                                println!(
                                    "Got message about someone that wanted data: {:?}",
                                    hostname
                                );
                                let _res = serde_json::to_writer(
                                    &stream,
                                    &self.observation_data.get(&hostname),
                                );
                            }
                            Message::SuicideNote(segment) => {
                                println!("Got a suicide note from {:?}", segment);
                                if let Some(index) = self.current_segments
                                    .iter()
                                    .position(|s| &s.hostname == &segment.hostname)
                                {
                                    self.current_segments.remove(index);
                                    self.cur_num_segments -= 1;
                                }
                            }
                            Message::GatheringCompleted => {
                                println!("Got message that we are completed!");
                                self.cur_num_segments = self.max_num_segments;
                                println!(
                                    "Setting current number of segments such that we should die"
                                );
                            }
                        }
                    } else {
                        println!("Error determining message...");
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if now.elapsed() > timeout {
                        println!("Would block and we have timed out!");
                        break;
                    }
                }
                Err(e) => {
                    println!("Unable to accept connection: {:?}", e);
                }
            }
        }
    }

    /// Send suicide note
    pub fn send_suicide_note(&self) {
        for host in self.current_segments.iter().take(5) {
            if host.relationship == TreeState::This {
                continue;
            }

            let timeout = Duration::from_secs(1);
            let hostname = &host.hostname;
            let mut addr = format!(
                "{}:{}",
                hostname,
                self.calculate_port(hostname.as_bytes(), false)
            ).as_str()
                .to_socket_addrs()
                .expect("Unable to resolve hostname to IP");
            if let Ok(stream) = TcpStream::connect_timeout(
                &addr.next().expect("No IP's matching the hostname"),
                timeout,
            ) {
                let _res = serde_json::to_writer(
                    &stream,
                    &Message::SuicideNote(WormSegment::new(
                        TreeState::This,
                        &self.current_hostname,
                    )),
                );
                println!("Sent suicide note to {:?}", host);
            }
        }
    }

    /// Send the program spawning the client to wormgate to infect next host
    fn send_prog_to_host(&self, host: &str) {
        let client = reqwest::Client::new();
        let mut buf = Vec::with_capacity(100);
        let binary_name = env::current_exe().expect("Unable to get the current executable");
        let mut f = File::open(binary_name).expect("Error opening file");

        // Read binary file into buffer and post it to wormgate
        let _n = f.read_to_end(&mut buf).expect("Could not read file to end");
        let res = client
            .post(&format!(
                "http://{}:{}/worm_entrance",
                host, self.wormgate_port
            ))
            .body(buf)
            .send()
            .expect("Error sending message");

        println!("Post result: {:?}", res);
    }

    /// Calculate the port of a specific hostname
    fn calculate_port(&self, hostname: &[u8], state_transfer: bool) -> u64 {
        let mut hasher = DefaultHasher::default();

        hasher.write(hostname);

        /* Make sure port is 16 bit and greater or equal than 1024 */
        if state_transfer {
            (hasher.finish() & 0xffff) | 1024
        } else {
            ((hasher.finish() & 0xffff) | 1024) ^ 1
        }
    }

    /// Send the Worm state to a listening worm segment
    fn send_data_to_host(&mut self, host: &str) {
        let _client = reqwest::Client::new();
        let port = self.calculate_port(host.as_bytes(), true);

        // Update Worm state before sending it
        self.cur_num_segments += 1;
        self.current_segments
            .push(WormSegment::new(TreeState::Child, host));

        println!("Sending data to: {}:{}", host, port);
        if let Ok(stream) = TcpStream::connect(&format!("{}:{}", host, port)) {
            let _res = serde_json::to_writer(stream, &self);
        } else {
            println!("Unable to connect - probably already infected");
        }
    }

    /// Send the program and Worm state to the specified host
    pub fn send_to_host(&mut self, host: &str) {
        self.send_prog_to_host(host);
        let hundred_ms = Duration::from_millis(100);
        thread::sleep(hundred_ms);
        self.send_data_to_host(host);
    }

    /// Send program and Worm state to a random host which we don't have data from
    pub fn send_to_random_host(&mut self) {
        let mut send_host = None;
        for host in &self.hosts_to_ovserve {
            println!("Checking if {:?} has been infected", host);
            if !self.observation_data.contains_key(host)
                && !self.current_segments
                    .iter()
                    .any(|ref h| &h.hostname == host)
            {
                println!("{:?} has not been infected - lets go!", host);
                send_host = Some(host.clone());
                break;
            }
        }
        if let Some(host) = send_host {
            self.send_to_host(&host);

            // Gossip about it to some other host - with a timeout
            for gossip_host in self.current_segments.iter().take(5) {
                if gossip_host.hostname == self.current_hostname {
                    continue;
                }
                println!("Gossip host: {:?}", gossip_host);
                let msg = Message::NewSegment(WormSegment::new(TreeState::Child, &host));
                let timeout = Duration::from_secs(1);
                let mut addr = format!(
                    "{}:{}",
                    &gossip_host.hostname,
                    self.calculate_port(gossip_host.hostname.as_bytes(), false)
                ).as_str()
                    .to_socket_addrs()
                    .expect("Unable to resolve hostname to IP");
                if let Ok(stream) = TcpStream::connect_timeout(
                    &addr.next().expect("No IP's matching the hostname"),
                    timeout,
                ) {
                    let _res = serde_json::to_writer(&stream, &msg);
                }
            }
        } else {
            println!("Could not find a free host");
        }
    }

    /// Return data to wormgate on current host
    pub fn return_data(&self) {
        let client = reqwest::Client::new();
        let res = client
            .post(&format!(
                "http://localhost:{}/observation_data",
                self.wormgate_port
            ))
            .json(&self.observation_data)
            .send()
            .expect("Error uploading data");
        println!("Uploaded data to wormgate: {:?}", res);

        for segment in &self.current_segments {
            if segment.hostname == self.current_hostname {
                continue;
            }
            let msg = Message::GatheringCompleted;
            let timeout = Duration::from_secs(1);
            let hostname = &segment.hostname;
            let mut addr = format!(
                "{}:{}",
                hostname,
                self.calculate_port(hostname.as_bytes(), false)
            ).as_str()
                .to_socket_addrs()
                .expect("Unable to resolve hostname to IP");
            if let Ok(stream) = TcpStream::connect_timeout(
                &addr.next().expect("No IP's matching the hostname"),
                timeout,
            ) {
                let _res = serde_json::to_writer(&stream, &msg);
            } else {
                println!("Unable to reach segment: {:?}", segment);
            }
        }
    }

    /// Determine if we have all data we should have before returning it
    pub fn is_finished(&self) -> bool {
        self.hosts_to_ovserve
            .iter()
            .all(|ref host| self.observation_data.contains_key(host.as_str()))
    }

    /// Query missing data based on known segments
    pub fn query_missing_data(&mut self) {
        // Iterate over known segment
        for segment in &self.current_segments {
            println!("Want to query segment: {:?}", segment);
            // If we are missing data from any of them - ask for it
            if !self.observation_data.contains_key(&segment.hostname) {
                println!("Querying segment: {:?} for observation", segment);
                let msg = Message::WantData(segment.hostname.clone());
                let timeout = Duration::from_secs(1);
                let hostname = &segment.hostname;
                let mut addr = format!(
                    "{}:{}",
                    hostname,
                    self.calculate_port(hostname.as_bytes(), false)
                ).as_str()
                    .to_socket_addrs()
                    .expect("Unable to resolve hostname to IP");
                if let Ok(stream) = TcpStream::connect_timeout(
                    &addr.next().expect("No IP's matching the hostname"),
                    timeout,
                ) {
                    println!("Connected..");
                    let _res = serde_json::to_writer(&stream, &msg);
                    stream
                        .shutdown(Shutdown::Write)
                        .expect("Write Shutdown failed");
                    println!("Sent WantData..waiting for observation");
                    if let Ok(observation) = serde_json::from_reader(&stream) {
                        let observation: String = observation;
                        self.observation_data
                            .insert(segment.hostname.clone(), observation);
                    } else {
                        println!("Could not parse stuff into a String");
                    }
                } else {
                    println!("Tried to connect to {:?} but it timed out", segment);
                }
            }
        }
    }
}

/// Daemonize current process
fn daemonize() {
    let this = env::current_exe().expect("Unable to get the current executable");
    println!("This executable is: {:?}", this);

    unsafe {
        let action = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
        if let Err(e) = sigaction(Signal::SIGCHLD, &action) {
            println!("Unable to ignore SIGCHLD: {:?}", e);
        } else {
            println!("Ignored SIGCHLD - getting rid of zombies");
        }
    }
    match fork() {
        Ok(ForkResult::Parent { child, .. }) => {
            println!(
                "Continuing execution in parent process, the new child has pid: {}",
                child
            );
        }
        Ok(ForkResult::Child) => {
            println!("I am the child process!");
            if let Ok(pid) = setsid() {
                println!("Setsid gave pid {}", pid);
            } else {
                println!("Setsid failed :(");
            }

            let c_self =
                CString::new(this.to_str().expect("No string?")).expect("Error creating C string");
            let c_arg = CString::new("-l").expect("Error creating C string");
            let c_new_name = CString::new("ls").expect("Error creating C string");
            match execv(&c_self, &[c_new_name, c_arg]) {
                Ok(_) => println!("Everything is a-okay: I completed execv!"),
                Err(e) => println!("Error executing execv: {}", e),
            }
        }
        Err(_) => println!("Fork failed :("),
    }
}

/// Number of arguments determine if the process is daemonized
fn is_daemonized() -> bool {
    env::args().len() > 1
}

/// Determine which port to bind to on current host
fn get_listen_port(state_transfer: bool) -> u64 {
    let mut buf = vec![0; 50];
    let hostname = gethostname(&mut buf)
        .expect("Error getting hostname")
        .to_str()
        .expect("Error using hostname as str")
        .split(".")
        .next()
        .expect("Error using hostname as str");
    get_send_port(hostname.as_bytes(), state_transfer)
}

/// Determine port to send data to at specified host name
fn get_send_port(hostname: &[u8], state_transfer: bool) -> u64 {
    let mut hasher = DefaultHasher::default();

    hasher.write(hostname);

    /* Make sure port is 16 bit and greater or equal than 1024 */
    if state_transfer {
        (hasher.finish() & 0xffff) | 1024
    } else {
        ((hasher.finish() & 0xffff) | 1024) ^ 1
    }
}

/// Listen for either the initial connection or a worm from parent segment
/// Update worm segment status after receiving it from parent
fn listen_for_worm() -> Result<Worm, &'static str> {
    let mut buf = vec![0; 50];
    let hostname = gethostname(&mut buf)
        .expect("Error getting hostname")
        .to_str()
        .expect("Error using hostname as str")
        .split(".")
        .next()
        .expect("Error using hostname as str");
    println!("Listening at {}:{}", hostname, get_listen_port(true));

    let listener = TcpListener::bind(format!("{}:{}", hostname, get_listen_port(true)))
        .expect("Error binding to port when listening for worm");

    /* Accept TCP connection */
    if let Ok((stream, addr)) = listener.accept() {
        println!("Got some data from {:?}", addr);
        /* Read shits from TCP stream */
        if let Ok(worm) = serde_json::from_reader(stream) {
            let mut worm: Worm = worm;
            println!("Deserialized worm data from stream");

            // Update worm segment data by calling the method for converting segment status
            worm.current_hostname = hostname.to_string();
            worm.current_segments = worm.current_segments
                .iter()
                .map(|segment| segment.send_to(&WormSegment::new(TreeState::Child, hostname)))
                .collect();

            Ok(worm)
        /* No worm, but a start command */
        } else {
            println!("Unable to deserialize worm data - must be initial segment");
            let file = File::open("hosts").expect("Unable to open hosts file");
            let mut reader = BufReader::new(file);
            let mut worm_port = String::new();
            let _nread = reader
                .read_line(&mut worm_port)
                .expect("Unable to read port number");
            println!("Worm port: {}", &worm_port);
            let worm_port = worm_port
                .trim()
                .parse::<u16>()
                .expect("Unable to parse port number");

            /* Parse hostnames and create worm */
            let mut hostnames: Vec<String> = vec![String::from(hostname)];
            for line in reader.lines() {
                if let Ok(line) = line {
                    hostnames.push(line);
                }
            }
            Ok(Worm::new(hostnames.len(), worm_port, hostnames))
        }
    } else {
        Err("Could not read from TCP stream")
    }
}

fn main() {
    if is_daemonized() {
        println!(
            "\nThis is now daemonized and was started with args: {:?}",
            env::args()
        );

        /* Listen for worm or initial message */
        println!("Listening for a worm!");
        let mut worm = listen_for_worm().expect("Unable to create worm");
        println!("Worm is: {:?}", worm);

        /* Get data from wormgate if we don't have it */
        if !worm.observation_data.contains_key(&worm.current_hostname) {
            worm.get_data();
        }

        /* Have we retrieved all data items */
        let mut suicide_counter = 0;
        loop {
            if worm.is_finished() {
                println!("Finished gathering all data items");
                if worm.current_hostname == worm.initial_hostname {
                    println!("Finally back home - should return data");
                    worm.return_data();
                    println!("Returned data - will die now");
                    return;
                } else {
                    println!("Need to relocate to initial host");
                    let host = worm.initial_hostname.clone();
                    worm.send_to_host(&host);
                    return;
                }
            } else {
                if !worm.should_infect() {
                    suicide_counter += 3;
                    println!("Suicide counter: {}", suicide_counter);
                    if suicide_counter >= 5 {
                        println!("Worm {:?} should infect {:?}", worm, worm.should_infect());
                        println!("Should not infect - I'll just die and send a message about it");
                        worm.send_suicide_note();
                        return;
                    } else {
                        println!("Suicide counter too low - listening for gossip - other suicides");
                        worm.listen_for_gossip();
                    }
                } else {
                    println!("Reset suicide counter - don't want to die anymore");
                    suicide_counter = 0;
                }

                /* If we should infect another host, do it */
                println!("Worm data: {:?}", worm);
                match rand::random::<u8>() % 3 {
                    0 => {
                        println!("Infecting another random host and gossiping about it");
                        worm.send_to_random_host();
                        println!("Sent myself to a random host!");
                    }
                    1 => {
                        println!("Listening for gossip from other hosts");
                        worm.listen_for_gossip();
                        println!("Gossip hour complete..");
                    }
                    2 => {
                        println!("Want to query for data");
                        worm.query_missing_data();
                        println!("Queried data");
                    }
                    _ => {
                        println!("A random number modulo 2 should never be anything but 0 or 1");
                    }
                }
            }
        }
    // Stealth is great so let's daemonize and stuff
    } else {
        daemonize();
    }
    println!("Goodbye from me... :)");
}
