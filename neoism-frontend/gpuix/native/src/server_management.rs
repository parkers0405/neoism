//! Host/server side effects stay off the render thread and reuse desktop hosting.
use neoism_agent_service_api::server_registry::{registry_directory,HostedServerSpec,ServerRegistry,SavedServer};
use serde_json::{json,Value};
use std::{collections::HashMap,path::PathBuf,sync::{Mutex,OnceLock,mpsc},io::Write};
#[derive(Default)]
struct Servers{next:u64,jobs:HashMap<u64,mpsc::Receiver<Result<Value,String>>>,fast:HashMap<String,crate::fast_servers::FastHost>,joins:HashMap<String,Value>}
static STORE:OnceLock<Mutex<Servers>>=OnceLock::new();
fn store()->&'static Mutex<Servers>{STORE.get_or_init(||Mutex::new(Servers::default()))}
fn text<'a>(v:&'a Value,key:&str)->&'a str{v[key].as_str().unwrap_or("")}
fn optional<'a>(v:&'a Value,key:&str)->Option<&'a str>{v[key].as_str().filter(|v|!v.is_empty())}
fn registry()->Result<ServerRegistry,String>{ServerRegistry::load(registry_directory()).map_err(|e|e.to_string())}
// The public listing reads only the atomically replaced metadata file.
// It never needs credentials or a writer lock while a host/save job commits.
fn saved_entries()->Result<Vec<SavedServer>,String>{
 let path=registry_directory().join("servers.json");
 let bytes=match std::fs::read(path){Ok(bytes)=>bytes,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>return Ok(Vec::new()),Err(error)=>return Err(error.to_string())};
 let data:Value=serde_json::from_slice(&bytes).map_err(|e|e.to_string())?;
 serde_json::from_value(data.get("servers").cloned().unwrap_or_else(||json!([]))).map_err(|e|e.to_string())
}
fn selection(registry:&ServerRegistry,id:&str)->Result<Value,String>{let server=registry.server(id).ok_or("Saved server no longer exists")?;Ok(json!({"id":id,"name":server.name,"endpoint":server.endpoint,"pairingToken":registry.token(id),"hosted":server.hosted.is_some()}))}
fn host(spec:&HostedServerSpec,password:Option<&str>)->Result<(),String>{
 std::fs::create_dir_all(&spec.state_dir).map_err(|e|e.to_string())?;
 let mut command=std::process::Command::new(crate::service_process::standalone_executable().map_err(|e|e.to_string())?);
 command.args(["--neoism-internal-hosted-workspace-daemon","--addr",&format!("0.0.0.0:{}",spec.port),"--no-unix-socket","--state-dir"]).arg(&spec.state_dir).arg("--workspace").arg(&spec.workspace_dir);
 command.env_remove("NEOISM_FAST_READINESS_NONCE").env("NEOISM_REQUIRE_AUTH",if spec.require_auth{"1"}else{"0"});
 if let Some(password)=password{command.env("NEOISM_DAEMON_TOKEN",password);}else{command.env_remove("NEOISM_DAEMON_TOKEN");}
 #[cfg(unix)]{use std::os::unix::process::CommandExt;unsafe{command.pre_exec(||{if libc::setsid()<0{return Err(std::io::Error::last_os_error());}Ok(())});}}
 crate::service_process::launch_persistent_host(&mut command,spec.port,password,||{
  let mut file=tempfile::NamedTempFile::new_in(&spec.state_dir)?;file.write_all(&serde_json::to_vec_pretty(spec)?)?;file.as_file().sync_all()?;file.persist(spec.state_dir.join("hosted-spec.json")).map_err(|e|e.error)?;Ok(())
 }).map(|_|()).map_err(|e|e.to_string())
}
/// Only the exact persistent daemon recipe may be stopped, including after a
/// frontend restart. Never signal a process just because it owns the port.
fn stop_hosted(spec:&HostedServerSpec){
 #[cfg(target_os="linux")]
 if let Ok(entries)=std::fs::read_dir("/proc"){
  for entry in entries.flatten(){
   let Ok(pid)=entry.file_name().to_string_lossy().parse::<i32>()else{continue;};
   let Ok(exe)=std::fs::read_link(entry.path().join("exe"))else{continue;};
   let name=exe.file_name().and_then(|name|name.to_str()).unwrap_or("");
   if !matches!(name.trim_end_matches(" (deleted)"),"neoism-workspace-daemon"|"neoism-workspace-daemon-grid"){continue;}
   let Ok(bytes)=std::fs::read(entry.path().join("cmdline"))else{continue;};
   let args=bytes.split(|byte|*byte==0).map(String::from_utf8_lossy).collect::<Vec<_>>();
   let state=spec.state_dir.to_string_lossy();let address=format!("0.0.0.0:{}",spec.port);
   if args.iter().any(|arg|arg=="--neoism-internal-hosted-workspace-daemon")
    && args.windows(2).any(|pair|pair[0]=="--state-dir"&&pair[1]==state)
    && args.windows(2).any(|pair|pair[0]=="--addr"&&pair[1]==address){unsafe{libc::kill(pid,libc::SIGTERM);}}
  }
 }
}
fn work(v:Value)->Result<Value,String>{
 match text(&v,"action"){
  "save"=>{let mut registry=registry()?;let id=text(&v,"serverId");let server=if id.is_empty(){registry.add(text(&v,"address"),optional(&v,"name"),optional(&v,"password"))?}else{registry.update(id,text(&v,"address"),optional(&v,"name"),optional(&v,"password"))?};selection(&registry,&server.id)},
  "remove"=>{let id=text(&v,"serverId");let mut state=store().lock().map_err(|_|"Server store unavailable")?;if let Some(mut fast)=state.fast.remove(id){fast.stop();}state.joins.remove(id);drop(state);let mut registry=registry()?;let hosted=registry.hosted_spec(id);registry.remove(id).map_err(|e|e.to_string())?;if let Some(spec)=hosted{stop_hosted(&spec);}Ok(json!({"removed":true}))},
  "join-fast"=>{let join=crate::fast_servers::normalize_fast_join(text(&v,"address").into(),optional(&v,"password").map(str::to_owned))?;if join.token.as_ref().is_none_or(|token|token.trim().is_empty()){return Err("Enter the host's password or an invite containing access".into());}let id=format!("fast-{}",uuid::Uuid::new_v4());let result=json!({"id":id,"name":optional(&v,"name").unwrap_or(&join.name),"endpoint":join.endpoint,"pairingToken":join.token,"authToken":join.token,"temporary":true});store().lock().map_err(|_|"Server store unavailable")?.joins.insert(id,result.clone());Ok(result)},
  "host-fast"=>{let hosted=crate::fast_servers::create_fast_server(text(&v,"directory").into(),optional(&v,"name").map(str::to_owned),optional(&v,"password").map(str::to_owned),optional(&v,"agentServer").map(str::to_owned).or_else(||std::env::var("NEOISM_AGENT_SERVER").ok()).unwrap_or_else(||"http://127.0.0.1:4096".into()))?;let result=json!({"id":hosted.id,"name":hosted.name,"endpoint":hosted.endpoint,"pairingToken":hosted.token,"authToken":hosted.token,"temporary":true,"hosted":true,"invite":hosted.public_url});store().lock().map_err(|_|"Server store unavailable")?.fast.insert(hosted.id.clone(),hosted);Ok(result)},
  "host"=>{let raw=text(&v,"directory");let directory=if raw=="~"{dirs::home_dir().ok_or("Cannot resolve home folder")?}else if let Some(rest)=raw.strip_prefix("~/"){dirs::home_dir().ok_or("Cannot resolve home folder")?.join(rest)}else{PathBuf::from(raw)};let directory=std::fs::canonicalize(directory).map_err(|_|"Project folder does not exist")?;if !directory.is_dir(){return Err("Project folder is not a directory".into());}let port=(9877..9900).find(|port|std::net::TcpListener::bind(("0.0.0.0",*port)).is_ok()).ok_or("No free hosted-server port between 9877 and 9899")?;let spec=HostedServerSpec{state_dir:registry_directory().join("hosted-servers").join(port.to_string()),workspace_dir:directory,port,require_auth:optional(&v,"password").is_some()};host(&spec,optional(&v,"password"))?;let saved=(||{let mut registry=registry()?;let server=registry.add_hosted(&format!("ws://127.0.0.1:{port}/session"),optional(&v,"name"),optional(&v,"password"),spec.clone())?;selection(&registry,&server.id)})();if saved.is_err(){stop_hosted(&spec);}saved},
  "rehost"=>{let registry=registry()?;let id=text(&v,"serverId");let spec=registry.hosted_spec(id).ok_or("This server is not hosted on this machine")?;host(&spec,registry.token(id))?;selection(&registry,id)},
  _=>Err("Unknown server action".into())
 }
}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{
 let op=text(v,"op");if !op.starts_with("server."){return None;}
 Some((||match op{
  "server.start"=>{let input=v.clone();let(tx,rx)=mpsc::channel();let mut state=store().lock().map_err(|_|"Server store unavailable")?;state.next+=1;let id=state.next;state.jobs.insert(id,rx);drop(state);std::thread::spawn(move||{let _=tx.send(work(input));});Ok(json!({"id":id,"phase":"pending"}))},
  "server.poll"=>{let id=v["id"].as_u64().unwrap_or(0);let mut state=store().lock().map_err(|_|"Server store unavailable")?;let rx=state.jobs.get(&id).ok_or("Unknown server request")?;match rx.try_recv(){Ok(result)=>{state.jobs.remove(&id);match result{Ok(result)=>Ok(json!({"phase":"ready","result":result})),Err(message)=>Ok(json!({"phase":"failed","message":message}))}},Err(mpsc::TryRecvError::Empty)=>Ok(json!({"phase":"pending"})),Err(_)=>{state.jobs.remove(&id);Err("Server worker stopped".into())}}},
  "server.list"=>{let saved=saved_entries()?;let mut entries=saved.iter().filter(|s|!s.agent_api).map(|s|json!({"id":s.id,"name":s.name,"address":s.endpoint,"hosted":s.hosted.is_some(),"temporary":false})).collect::<Vec<_>>();let state=store().lock().map_err(|_|"Server store unavailable")?;for host in state.fast.values(){entries.push(json!({"id":host.id,"name":host.name,"address":host.endpoint,"temporary":true,"hosted":true,"online":host.is_running()}));}for join in state.joins.values(){entries.push(json!({"id":join["id"],"name":join["name"],"address":join["endpoint"],"temporary":true,"hosted":false}));}Ok(json!({"entries":entries}))},
  "server.resolve"|"server.edit"=>{let id=text(v,"serverId");let state=store().lock().map_err(|_|"Server store unavailable")?;if let Some(host)=state.fast.get(id){if !host.is_running(){return Err("Fast Server has stopped".into());}return Ok(json!({"id":host.id,"name":host.name,"endpoint":host.endpoint,"pairingToken":host.token,"authToken":host.token,"temporary":true,"hosted":true,"invite":host.public_url}));}if let Some(join)=state.joins.get(id){return Ok(join.clone());}drop(state);selection(&registry()?,id)},
  "server.shutdown"=>{let mut state=store().lock().map_err(|_|"Server store unavailable")?;for (_,mut host) in state.fast.drain(){host.stop_and_wait();}state.joins.clear();Ok(json!({}))},
  _=>Err("Unknown server operation".into())
 })())
}
