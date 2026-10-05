#![cfg(target_os = "linux")]
// Run fork/exec evidence tests in their own process. Fork can momentarily inherit
// another unit test's advisory lock even when every fd closes on exec.
#[path = "../src/process_identity.rs"]
mod process_identity;
use process_identity::sunshine_generation;

mod tests {
    use super::sunshine_generation;
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
    };
    struct Listener(Child);
    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn start(binary: &std::path::Path, port: u16) -> (Listener, u16) {
        let mut child = Command::new(binary)
            .arg(port.to_string())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        (Listener(child), line.trim().parse().unwrap())
    }
    #[test]
    fn process_evidence_belongs_to_the_api_listener_and_changes_on_restart() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("listener.c");
        let binary = root.path().join("sunshine");
        std::fs::write(
            &source,
            r#"
#include <arpa/inet.h>
#include <stdlib.h>
#include <stdio.h>
#include <unistd.h>
int main(int argc, char **argv) {
 int fd=socket(AF_INET,SOCK_STREAM,0), one=1;
 setsockopt(fd,SOL_SOCKET,SO_REUSEADDR,&one,sizeof(one));
 struct sockaddr_in address={0}; address.sin_family=AF_INET;
 address.sin_addr.s_addr=htonl(INADDR_LOOPBACK); address.sin_port=htons(atoi(argv[1]));
 if(bind(fd,(void*)&address,sizeof(address)) || listen(fd,4)) return 1;
 socklen_t len=sizeof(address); getsockname(fd,(void*)&address,&len);
 printf("%u\n",ntohs(address.sin_port)); fflush(stdout);
 for(;;) pause();
}
"#,
        )
        .unwrap();
        assert!(
            Command::new("cc")
                .arg(&source)
                .arg("-o")
                .arg(&binary)
                .status()
                .unwrap()
                .success()
        );
        let (first, port) = start(&binary, 0);
        let (_unrelated, unrelated_port) = start(&binary, 0);
        let generation = sunshine_generation(port).unwrap();
        assert_eq!(
            generation
                .split(':')
                .nth(1)
                .unwrap()
                .parse::<u32>()
                .unwrap(),
            first.0.id()
        );
        assert_ne!(generation, sunshine_generation(unrelated_port).unwrap());
        drop(first);
        assert!(sunshine_generation(port).is_none());
        let (_second, _) = start(&binary, port);
        assert_ne!(generation, sunshine_generation(port).unwrap());
    }
}
