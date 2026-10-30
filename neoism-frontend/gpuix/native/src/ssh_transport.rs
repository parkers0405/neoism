//! Nonblocking host adapter around the original desktop SSH daemon transport.
//! The terminal stays Ghostty; its PTY is owned by the daemon behind this tunnel.
#[allow(dead_code)]
#[path = "ssh_hosts.rs"]
pub(crate) mod original;
#[allow(dead_code)]
#[path = "system_registry.rs"]
mod registry;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{atomic::{AtomicU32, Ordering}, Arc, Mutex, OnceLock};

enum State { Pending, Ready(original::DaemonAttach), Failed(String), Closed }
struct Job { state: Mutex<State>, target:String, args:Vec<String> }
static JOBS: OnceLock<Mutex<HashMap<u32, Arc<Job>>>> = OnceLock::new();
static NEXT: AtomicU32 = AtomicU32::new(1);
static WORKERS: OnceLock<Mutex<Vec<std::thread::JoinHandle<()>>>> = OnceLock::new();
fn workers() -> &'static Mutex<Vec<std::thread::JoinHandle<()>>> { WORKERS.get_or_init(Default::default) }
fn jobs() -> &'static Mutex<HashMap<u32, Arc<Job>>> { JOBS.get_or_init(Default::default) }
fn hosts()->Vec<Value>{original::available_hosts().into_iter().map(|host|json!({"target":host.alias,"hostname":host.hostname,"user":host.user,"port":host.port})).collect()}
fn arguments(v:&Value)->Result<Vec<String>,String>{
    let args=v["args"].as_array().or_else(||v["ssh_args"].as_array()).into_iter().flatten().map(|value|value.as_str().map(str::to_owned).ok_or_else(||"Invalid SSH argument".to_owned())).collect::<Result<Vec<_>,_>>()?;
    if args.len()%2!=0 || args.iter().any(|arg|arg.chars().any(char::is_control)) || original::safe_connection_args(&args).len()!=args.len(){return Err("SSH options must be supported flag/value pairs".into());}
    Ok(args)
}
fn target(v:&Value)->Result<String,String>{
    let target=v["target"].as_str().unwrap_or("");
    if target.is_empty() || target.starts_with('-') || target.chars().any(|c|c.is_whitespace()||c.is_control()) {
        return Err("Enter an SSH host alias or user@host".into());
    }
    Ok(target.to_owned())
}
pub fn dispatch(v:&Value)->Option<Result<Value,String>> {
    let op=v["op"].as_str()?;
    if !op.starts_with("ssh."){return None;}
    Some((||{
        match op {
            "ssh.identity" => {
                let target=target(v)?;let args=arguments(v)?;
                Ok(json!({"workspaceId":original::quick_ssh_workspace_id_with_args(&target,&args)}))
            }
            "ssh.systems" | "ssh.systems.list" => {
                let registry=registry::SystemRegistry::load().map_err(|error|error.to_string())?;
                let jobs=jobs().lock().map_err(|_|"SSH transport lock unavailable")?;
                let systems=registry.systems().iter().map(|system|{
                    let mut entry=json!(system);let mut connection="disconnected";
                    for job in jobs.values(){
                        if !registry::same_system_route(&system.target,&system.ssh_args,&job.target,&job.args){continue;}
                        if let Ok(mut state)=job.state.lock(){
                            if let State::Ready(attach)=&mut *state{if !attach.is_running(){*state=State::Failed("SSH connection closed".into());}}
                            match &*state{State::Ready(_)=>connection="connected",State::Pending=>connection="connecting",State::Failed(message)=>{connection="error";entry["error"]=json!(message);},State::Closed=>{}}
                        }
                        if connection=="connected"{break;}
                    }
                    entry["connection"]=json!(connection);entry
                }).collect::<Vec<_>>();
                Ok(json!({"systems":systems,"hosts":hosts()}))
            }
            "ssh.systems.get" => {
                let registry=registry::SystemRegistry::load().map_err(|error|error.to_string())?;
                Ok(json!({"system":registry.system(v["systemId"].as_str().unwrap_or("")).ok_or("Saved system not found")?}))
            }
            "ssh.systems.add" | "ssh.systems.upsert" => {
                let mut registry=registry::SystemRegistry::load().map_err(|error|error.to_string())?;
                let saved=registry.upsert(v["name"].as_str().unwrap_or(""),&target(v)?,arguments(v)?).map_err(|error|error.to_string())?;
                Ok(json!({"system":saved}))
            }
            "ssh.systems.remove" => {
                let mut registry=registry::SystemRegistry::load().map_err(|error|error.to_string())?;
                registry.remove(v["systemId"].as_str().unwrap_or("")).map_err(|error|error.to_string())?;
                Ok(json!({"removed":true}))
            }
            "ssh.systems.set-enabled" => {
                let mut registry=registry::SystemRegistry::load().map_err(|error|error.to_string())?;
                let system=registry.set_enabled(v["systemId"].as_str().unwrap_or(""),v["enabled"].as_bool().unwrap_or(false)).map_err(|error|error.to_string())?;
                Ok(json!({"system":system}))
            }
            "ssh.hosts" => Ok(json!(hosts())),
            "ssh.start" => {
                let (target,args)=if let Some(id)=v["systemId"].as_str(){
                    let registry=registry::SystemRegistry::load().map_err(|error|error.to_string())?;
                    let system=registry.system(id).ok_or("Saved system not found")?;
                    (system.target.clone(),system.ssh_args.clone())
                }else{(target(v)?,arguments(v)?)};
                let id=NEXT.fetch_add(1,Ordering::Relaxed);
                let job=Arc::new(Job{state:Mutex::new(State::Pending),target:target.clone(),args:args.clone()});
                jobs().lock().map_err(|_|"SSH transport lock unavailable")?.insert(id,job.clone());
                let worker=std::thread::spawn(move||{
                    let result=original::attach_workspace_over_ssh(&target,args);
                    if let Ok(mut state)=job.state.lock(){
                        // Cancelling must drop a late successful tunnel instead
                        // of resurrecting it after the user leaves the modal.
                        if matches!(*state,State::Pending){*state=match result{Ok(attach)=>State::Ready(attach),Err(error)=>State::Failed(error.to_string())};}
                    }
                });
                let mut workers=workers().lock().map_err(|_|"SSH worker lock unavailable")?;workers.retain(|worker|!worker.is_finished());workers.push(worker);
                Ok(json!({"id":id,"phase":"pending"}))
            }
            "ssh.poll" => {
                let id=v["id"].as_u64().unwrap_or(0) as u32;
                let job=jobs().lock().map_err(|_|"SSH transport lock unavailable")?.get(&id).cloned().ok_or("SSH attachment no longer exists")?;
                let mut state=job.state.lock().map_err(|_|"SSH attachment lock unavailable")?;
                if let State::Ready(attach)=&mut *state{if !attach.is_running(){*state=State::Failed("SSH connection closed".into());}}
                Ok(match &*state{
                    State::Pending=>json!({"phase":"pending"}),
                    State::Ready(attach)=>json!({"phase":"ready","endpoint":attach.daemon_url,"authToken":attach.credential,"workspaceId":attach.workspace_id,"target":attach.alias}),
                    State::Failed(message)=>json!({"phase":"failed","message":message}),
                    State::Closed=>json!({"phase":"closed"}),
                })
            }
            "ssh.shutdown" => {
                let active=jobs().lock().map_err(|_|"SSH transport lock unavailable")?.drain().map(|(_,job)|job).collect::<Vec<_>>();
                for job in active{*job.state.lock().map_err(|_|"SSH attachment lock unavailable")?=State::Closed;}
                // A cancelled bootstrap can still own its ssh child on the
                // background stack. Reap it before the process exits; ordinary
                // modal cancellation stays nonblocking through ssh.close.
                let workers=std::mem::take(&mut *workers().lock().map_err(|_|"SSH worker lock unavailable")?);
                for worker in workers{let _=worker.join();}
                Ok(json!({"closed":true}))
            }
            "ssh.close" => {
                let id=v["id"].as_u64().unwrap_or(0) as u32;
                if let Some(job)=jobs().lock().map_err(|_|"SSH transport lock unavailable")?.remove(&id){
                    *job.state.lock().map_err(|_|"SSH attachment lock unavailable")?=State::Closed;
                }
                Ok(json!({"closed":true}))
            }
            _=>Err("Unknown SSH transport operation".into()),
        }
    })())
}
