extern crate nix;
extern crate reqwest;

#[macro_use]
extern crate serde_derive;

extern crate serde;
extern crate serde_json;

use nix::unistd::{execv, setsid, fork, gethostname, ForkResult};

use std::io::{BufReader, BufRead, Read};
use std::fs::File;
use std::net::{TcpListener, TcpStream};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::CString;
use std::vec::Vec;
use std::hash::Hasher;
use std::env;

#[derive(Deserialize, Serialize, Debug)]
enum TreeState {
    Child,
    Parent,
    Sibling,
    This
}

#[derive(Deserialize, Serialize, Debug)]
struct WormSegment {
    relationship: TreeState,
    hostname: String
}

#[derive(Deserialize, Serialize, Debug)]
struct Worm {
    initial_hostname: String,
    current_hostname: String,
    max_num_segments: usize,
    cur_num_segments: usize,
    observation_data: HashMap<String, String>,
    current_segments: Vec<WormSegment>,
    hosts_to_ovserve: Vec<String>
}

impl WormSegment {
    pub fn new(rel: TreeState, host: &str) -> WormSegment {
        WormSegment {
            relationship: rel,
            hostname: String::from(host)
        }
    }
}

impl Worm {
    /// Create a new worm with max number of segments and a list of hosts
    ///
    /// Should only be used the very first time a worm is created,
    /// and the rest should simply be sent.
    pub fn new(max_segments: usize, hosts: Vec<String>) -> Worm {
        let mut buf = vec![0; 50];
        let hostname = gethostname(&mut buf).expect("Error getting hostname")
            .to_str().expect("Error using hostname as str");

        Worm {
            initial_hostname: String::from(hostname),
            current_hostname: String::from(hostname),
            max_num_segments: max_segments,
            cur_num_segments: 1,
            observation_data: HashMap::new(),
            current_segments: vec!(WormSegment::new(TreeState::This, hostname)),
            hosts_to_ovserve: hosts
        }
    }

    /// Get data from wormgate on current host
    pub fn get_data(&mut self) {
        let map: HashMap<String, String> = reqwest::get("http://localhost:8000/observation_data")
            .expect("Error requesting observation data")
            .json().expect("Error parsing JSON");

        for (k, v) in &map {
            // Strip away port number from data
            self.observation_data.insert(k[..k.len()-5].to_string(), v.to_string());
        }
    }

    /// Determine if the worm should infect a new host
    pub fn should_infect(&self) -> bool {
        self.cur_num_segments < self.max_num_segments
    }

    /// Send the program spawning the client to wormgate to infect next host
    fn send_prog_to_host(&self, host: &str) {
        let client = reqwest::Client::new();
        let mut buf = Vec::with_capacity(100);
        let binary_name = env::current_exe().expect("Unable to get the current executable");
        let mut f = File::open(binary_name).expect("Error opening file");

        // Read binary file into buffer and post it to wormgate
        let _n = f.read_to_end(&mut buf).expect("Could not read file to end");
        let res = client.post(&format!("http://{}:8000/worm_entrance", host))
            .body(buf)
            .send().expect("Error sending message");

        println!("Post result: {:?}", res);
    }

    /// Calculate the port of a specific hostname
    fn calculate_port(&self, hostname: &[u8]) -> u64 {
        let mut hasher = DefaultHasher::default();

        hasher.write(hostname);

        /* Make sure port is 16 bit and greater or equal than 1024 */
        (hasher.finish() & 0xffff) | 1024
    }

    /// Send the Worm state to a listening worm segment
    fn send_data_to_host(&mut self, host: &str) {
        let _client = reqwest::Client::new();
        let port = self.calculate_port(host.as_bytes());

        // Update Worm state before sending it
        self.cur_num_segments += 1;

        println!("Sending data to: {}:{}", host, port);
        let stream = TcpStream::connect(&format!("{}:{}", host, port)).expect("Could not bind to socket");
        let _res = serde_json::to_writer(stream, &self);
    }

    /// Send the program and Worm state to the specified host
    pub fn send_to_host(&mut self, host: &str) {
        self.send_prog_to_host(host);
        self.send_data_to_host(host);
    }

    /// Send program and Worm state to a random host which we don't have data from
    pub fn send_to_random_host(&mut self) {
        let mut send_host = None;
        for host in &self.hosts_to_ovserve {
            println!("Checking if {:?} has been infected", host);
            if !self.observation_data.contains_key(host) {
                println!("{:?} has not been infected - lets go!", host);
                send_host = Some(host.clone());
                break;
            }
        }
        if let Some(host) = send_host {
            self.send_to_host(&host);
        } else {
            println!("Could not find a free host");
        }
    }

    /// Return data to wormgate on current host
    pub fn return_data(&self) {
        let client = reqwest::Client::new();
        let res = client.post("http://localhost:8000/observation_data")
            .json(&self.observation_data)
            .send().expect("Error uploading data");
        println!("Uploaded data to wormgate: {:?}", res);
    }

    /// Determine if we have all data we should have before returning it
    pub fn is_finished(&self) -> bool {
        self.hosts_to_ovserve.iter().all(|ref host| self.observation_data.contains_key(host.as_str()))
    }
}

/// Daemonize current process
fn daemonize() {
    let this = env::current_exe().expect("Unable to get the current executable");
    println!("This executable is: {:?}", this);

    match fork() {
        Ok(ForkResult::Parent { child, .. }) => {
            println!("Continuing execution in parent process, the new child has pid: {}", child);
        },
        Ok(ForkResult::Child) => {
            println!("I am the child process!");
            if let Ok(pid) = setsid() {
                println!("Setsid gave pid {}", pid);
            } else {
                println!("Setsid failed :(");
            }

            let c_self = CString::new(this.to_str().expect("No string?")).expect("Error creating C string");
            let c_arg = CString::new("-l").expect("Error creating C string");
            let c_new_name = CString::new("ls").expect("Error creating C string");
            match execv(&c_self, &[c_new_name, c_arg]) {
                Ok(_) => println!("Everything is a-okay: I completed execv!"),
                Err(e) => println!("Error executing execv: {}", e),
            }
        },
        Err(_) => println!("Fork failed :("),
    }
}

/// Number of arguments determine if the process is daemonized
fn is_daemonized() -> bool {
    env::args().len() > 1
}

/// Determine which port to bind to on current host
fn get_listen_port() -> u64 {
    let mut buf = vec![0; 50];
    let hostname = gethostname(&mut buf).expect("Error getting hostname");
    get_send_port(hostname.to_bytes())
}

/// Determine port to send data to at specified host name
fn get_send_port(hostname: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::default();

    hasher.write(hostname);

    /* Make sure port is 16 bit and greater or equal than 1024 */
    (hasher.finish() & 0xffff) | 1024
}

fn listen_for_worm() -> Result<Worm, &'static str> {
        let mut buf = vec![0; 50];
        let hostname = gethostname(&mut buf).expect("Error getting hostname")
            .to_str().expect("Error using hostname as str");
        println!("Listening at {}:{}", hostname, get_listen_port());

        let listener = TcpListener::bind(format!("{}:{}", hostname , get_listen_port())).expect("Error binding to port");

        /* Accept TCP connection */
        if let Ok((stream, addr)) = listener.accept() {
            println!("Got some data from {:?}", addr);
            /* Read shits from TCP stream */
            if let Ok(worm) = serde_json::from_reader(stream) {
                let mut worm: Worm = worm;
                println!("Deserialized worm data from stream");
                worm.current_hostname = hostname.to_string();
                worm.cur_num_segments += 1;
                Ok(worm)
            /* No worm, but a start command */
            } else {
                println!("Unable to deserialize worm data - must be initial segment");
                let file = File::open("hosts").expect("Unable to open hosts file");
                let mut reader = BufReader::new(file);

                /* Parse hostnames and create worm */
                let mut hostnames: Vec<String> = vec![String::from(hostname)];
                for line in reader.lines() {
                    if let Ok(line) = line {
                        hostnames.push(line);
                    }
                }
                Ok(Worm::new(hostnames.len(), hostnames))
            }
        } else {
            Err("Could not read from TCP stream")
        }

}

fn main() {
    if is_daemonized() {
        println!("\nThis is now daemonized and was started with args: {:?}", env::args());

        /* Listen for worm or initial message */
        println!("Listening for a worm!");
        let mut worm = listen_for_worm().expect("Unable to create worm");
        println!("Worm is: {:?}", worm);

        /* Have we retrieved all data items */
        if worm.is_finished() {
            println!("Finished gathering all data items");
            if worm.current_hostname == worm.initial_hostname {
                println!("Finally back home - should return data");
                worm.return_data();
                println!("Returned data - will die sooner or later");
            } else {
                println!("Need to relocate to initial host");
                let host = worm.initial_hostname.clone();
                worm.send_to_host(&host);
            }
        } else {
            /* Get data from wormgate */
            worm.get_data();
            /* If we should infect another host, do it */
            if worm.should_infect() {
                println!("Infecting another random host");
                worm.send_to_random_host();
            } else {
                println!("I don't know what to do anymore - I'll just die..");
            }
        }
    } else {
        daemonize();
    }
}
