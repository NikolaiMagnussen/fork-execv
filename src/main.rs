extern crate nix;

use nix::unistd::{execv, setsid, fork, gethostname, ForkResult};
use std::{thread, time};
use std::collections::hash_map::DefaultHasher;
use std::ffi::CString;
use std::hash::Hasher;
use std::env;

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

fn main() {
    if is_daemonized() {
        println!("This is now daemonized and was started with args: {:?}", env::args());

        // Get port for listening
        let port = get_listen_port();
        println!("Port number: {}", port);

        // Final shit - sleep to make sure we can see the command
        println!("Going to sleep for 20 seconds..");
        let twenty_seconds = time::Duration::new(20, 0);
        thread::sleep(twenty_seconds);
        println!("Sleeping done - exiting now! :-)");
    } else {
        daemonize();
    }
}
