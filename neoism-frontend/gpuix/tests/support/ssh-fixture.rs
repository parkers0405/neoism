// Standalone Windows SSH fixture: a real process with no shell/shebang
// dependency. The transport can terminate its actual forwarding process.
use std::{fs::{self,OpenOptions},io::{self,Write},net::{TcpListener,TcpStream},path::PathBuf,thread,time::Duration};
fn main()->io::Result<()>{
 let root=PathBuf::from(std::env::var_os("NEOISM_SSH_FIXTURE_DIR").expect("isolated fixture"));
 let args=std::env::args().skip(1).collect::<Vec<_>>();
 writeln!(OpenOptions::new().create(true).append(true).open(root.join("calls.jsonl"))?,"{args:?}")?;
 let Some(index)=args.iter().position(|arg|arg=="-L") else{
  if root.join("slow-credential").exists(){fs::write(root.join("credential-started"),"ready")?;thread::sleep(Duration::from_millis(500));}
  println!("fixture-remote-daemon-credential");return Ok(());
 };
 writeln!(OpenOptions::new().create(true).append(true).open(root.join("control-pids"))?,"{}",std::process::id())?;
 let port=args[index+1].split(':').next().unwrap().parse::<u16>().unwrap();
 let remote=fs::read_to_string(root.join("remote-port"))?.parse::<u16>().unwrap();
 for incoming in TcpListener::bind(("127.0.0.1",port))?.incoming(){
  let mut client=incoming?;let mut server=TcpStream::connect(("127.0.0.1",remote))?;
  let mut client_read=client.try_clone()?;let mut server_write=server.try_clone()?;
  thread::spawn(move||{let _=io::copy(&mut client_read,&mut server_write);});
  thread::spawn(move||{let _=io::copy(&mut server,&mut client);});
 }
 Ok(())
}
