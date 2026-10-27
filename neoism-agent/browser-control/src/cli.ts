import { startServer } from "./server.ts"
import { invoke } from "./client.ts"
import { pairingToken } from "./bridge.ts"
import { shortError, type Engine } from "./protocol.ts"
const command=process.argv[2] ?? "help", engine:Engine=process.argv[3] === "firefox" ? "firefox" : "chrome"
try {
  if(command === "serve") {
    const server=await startServer(engine)
    process.stderr.write(`Neoism ${engine} browser driver listening on 127.0.0.1:${server.port}\n`)
    const stop=()=>{void server.close().finally(()=>process.exit())}
    process.once("SIGINT",stop);process.once("SIGTERM",stop)
  } else if(command === "pair") {await invoke(engine,"status");process.stdout.write(`${await pairingToken(engine)}\n`)}
  else if(command === "status") process.stdout.write(`${JSON.stringify(await invoke(engine,"status"),null,2)}\n`)
  else if(command === "call") process.stdout.write(`${JSON.stringify(await invoke(engine,process.argv[4] ?? "status",JSON.parse(process.argv[5] ?? "{}")),null,2)}\n`)
  else process.stdout.write("Neoism browser: serve|pair|status [chrome|firefox]\nneoism-browser call [chrome|firefox] <tool> '<JSON arguments>'\n")
}catch(error){process.stderr.write(`${shortError(error)}\n`);process.exitCode=1}
