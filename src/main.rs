extern crate nix;
extern crate reqwest;

#[macro_use]
extern crate serde_derive;

extern crate serde;
extern crate serde_json;

use nix::unistd::{execv, setsid, fork, gethostname, ForkResult};

use std::io::{BufReader, BufRead, Read};
use std::fs::File;
use std::net::{TcpListener};
use std::{thread, time};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::CString;
use std::vec::Vec;
use std::hash::Hasher;
use std::env;

#[derive(Deserialize, Debug)]
struct ObservationData {
    #[serde(rename = "localhost:8000")]  // <-- this is a variant attribute
    data: String
}

#[derive(Deserialize, Serialize, Debug)]
struct Worm {
    initial_hostname: String,
    current_hostname: String,
    max_num_segments: usize,
    cur_num_segments: usize,
    observation_data: HashMap<String, String>,
    hosts_to_ovserve: Vec<String>
}

impl Worm {
    // Should only be called upon initial Worm creation
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
            hosts_to_ovserve: hosts
        }
    }

    pub fn get_data(&mut self) {
        let map: HashMap<String, String> = reqwest::get("http://localhost:8000/observation_data")
            .expect("Error requesting observation data")
            .json().expect("Error parsing JSON");

        for (k, v) in map.iter() {
            self.observation_data.insert(k.to_string(), v.to_string());
        }
    }

    pub fn should_infect(&self) -> bool {
        self.cur_num_segments < self.max_num_segments
    }

    fn send_prog_to_host(&self, host: &str) {
        let client = reqwest::Client::new();
        let mut buf = Vec::with_capacity(100);
        let binary_name = env::current_exe().expect("Unable to get the current executable");
        let mut f = File::open(binary_name).expect("Error opening file");

        let n = f.read_to_end(&mut buf).expect("Could not read file to end");

        let res = client.post(&format!("http://{}:8000/worm_entrance", host))
            .body(buf)
            .send().expect("Error sending message");
        println!("Post result: {:?}", res);
    }

    fn calculate_port(&self, hostname: &[u8]) -> u64 {
        let mut hasher = DefaultHasher::default();

        hasher.write(hostname);

        /* Make sure port is 16 bit and greater or equal than 1024 */
        (hasher.finish() & 0xffff) | 1024
    }

    fn send_data_to_host(&self, host: &str) {
        let client = reqwest::Client::new();
        let port = self.calculate_port(host.as_bytes());

        let res = client.post(&format!("http://{}:{}", host, port))
            .json(&self)
            .send().expect("Error sending message");
        println!("Sent data to host and got response: {:?}", res);
    }

    pub fn send_to_random_host(&self) {
        for host in self.hosts_to_ovserve.iter() {
            if self.observation_data.contains_key(host) == false {
                self.send_prog_to_host(&host);
                self.send_data_to_host(&host);

                return
            }
        }
    }
}

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

fn is_daemonized() -> bool {
    env::args().len() > 1
}

fn get_listen_port() -> u64 {
    let mut buf = vec![0; 50];
    let hostname = gethostname(&mut buf).expect("Error getting hostname");
    get_send_port(hostname.to_bytes())
}

fn get_send_port(hostname: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::default();

    hasher.write(hostname);

    /* Make sure port is 16 bit and greater or equal than 1024 */
    (hasher.finish() & 0xffff) | 1024
}

fn get_data_from_self() -> String {
    let map: HashMap<String, String> = reqwest::get("http://localhost:8000/observation_data")
        .expect("Error requesting observation data")
        .json().expect("Error parsing json");

    /* Return the value of the first key */
    let name = map.keys().next().expect("No keys are present..");
    String::from(map[name].as_str())
}

fn spread() {
    let client = reqwest::Client::new();
    let binary_name = env::current_exe().expect("Unable to get the current executable");
    println!("Binary name: {:?}", binary_name);

    let mut f = File::open(binary_name).expect("Error opening file");
    let mut buf = Vec::with_capacity(100);

    let n = f.read_to_end(&mut buf).expect("Could not read file to end");
    println!("Number of bytes read: {}", n);

    let res = client.post("http://localhost:8000/worm_entrance")
        .body(buf)
        .send().expect("Error sending message");
    println!("Post result: {:?}", res);
}



fn listen_for_worm() -> Result<Worm, &'static str> {
        let mut buf = vec![0; 50];
        let hostname = gethostname(&mut buf).expect("Error getting hostname")
            .to_str().expect("Error using hostname as str");
        println!("Listening at {}:{}", hostname, get_listen_port());

        let listener = TcpListener::bind(format!("{}:{}", hostname , get_listen_port())).expect("Error binding to port");

        /* Accept TCP connection */
        if let Ok((stream, _addr)) = listener.accept() {
            /* Read shits from TCP stream */
            if let Ok(worm) = serde_json::from_reader(stream) {
                Ok(worm)
            /* No worm, but a start command */
            } else {
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
        println!("This is now daemonized and was started with args: {:?}", env::args());

        /* Listen for worm or initial message */
        let mut worm = listen_for_worm().expect("Unable to create worm");
        println!("Worm is: {:?}", worm);

        worm.get_data();

        if worm.should_infect() {
            worm.send_to_random_host();
        }

        // Final shit - sleep to make sure we can see the command
        println!("Going to sleep..");
        let sleep_time = time::Duration::new(5, 0);
        thread::sleep(sleep_time);
        println!("Sleeping done - exiting now! :-)");
    } else {
        daemonize();
    }
}
