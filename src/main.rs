extern crate nix;

use nix::unistd::{execv, setsid, fork, ForkResult};
use std::{thread, time};
use std::ffi::CString;
use std::env;

fn main() {
    let args = env::args();
    let this = env::current_exe().expect("Unable to get the current executable");
    println!("This executable is: {:?}", this);

    println!("Length of argument list: {}", args.len());
    if args.len() == 1 {
        match fork() {
            Ok(ForkResult::Parent { child, .. }) => {
                println!("Continuing execution in parent process, the new child has pid: {}", child);
            },
            Ok(ForkResult::Child) => {
                println!("I am the child process!");
                if let Ok(pid) = setsid() {
                    println!("Pid: {}", pid);
                } else {
                    println!("Setsid failed :(");
                }
                println!("Execing itself..");
                let c_sleep = CString::new(this.to_str().expect("No string?")).expect("Error creating C string");
                let c_ten_sec = CString::new("-l").expect("Error creating C string");
                let c_new_name = CString::new("ls").expect("Error creating C string");
                match execv(&c_sleep, &[c_new_name, c_ten_sec]) {
                    Ok(_) => println!("Everything is a-okay: I completed execv!"),
                    Err(e) => println!("Error executing execv: {}", e),
                }
                let ten_seconds = time::Duration::new(10, 0);
                thread::sleep(ten_seconds);
                println!("I am done sleeping - exiting now");
            },
            Err(_) => println!("Fork failed :("),
        }
    } else {
        println!("This is probably spawned by some shit");
        for a in args {
            println!("{}", a);
        }
        println!("Going to sleep..");
        let ten_seconds = time::Duration::new(20, 0);
        thread::sleep(ten_seconds);
        println!("Sleeping done - exiting now! :)");
    }
}
