import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js"
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js"
import { z } from "zod"
import { invoke } from "./client.ts"
import { pairingToken } from "./bridge.ts"
import { selectedEngine, shortError } from "./protocol.ts"
import { guide } from "./guide.ts"

export async function main():Promise<void> {
  const engine=selectedEngine(), server=new McpServer({name:`neoism-browser-${engine}`,version:"0.2.0"})
  const session=z.string().optional()
  function tool(name:string,description:string,shape:Record<string,z.ZodType>,local?:()=>Promise<unknown>) {
    server.registerTool(name,{description,inputSchema:shape},async(args:any)=> {
      try {
        const value:any=local ? await local() : await invoke(engine,name,args)
        if(value?.type === "image")return {content:[{type:"image" as const,mimeType:value.mimeType,data:value.data}]}
        const text=typeof value === "string" ? value : JSON.stringify(value)
        return {content:[{type:"text" as const,text:text.slice(0,64000)}]}
      }catch(error){return {isError:true,content:[{type:"text" as const,text:shortError(error)}]}}
    })
  }
  tool("guide","Read Neoism browser workflow and supported capabilities",{},async()=>guide)
  tool("connect","Start Neoism's local relay and return the private pairing token for extension setup",{},async()=> {const status=await invoke(engine,"status");return {status,pairingToken:await pairingToken(engine),instructions:"Enter this token in Neoism's extension popup, connect, then attach the desired tab"}})
  tool("status","List attached tabs, connection state and sessions",{})
  tool("capabilities","Report this driver's actual supported features",{})
  tool("session_new","Create an isolated Neoism state/session namespace",{name:z.string().optional()})
  tool("session_list","List Neoism session names",{})
  tool("session_current","Get the selected session",{})
  tool("session_use","Select the session for subsequent calls",{session:z.string()})
  tool("session_adopt","Select an explicitly attached browser tab",{session,tabId:z.number().int()})
  tool("session_reset","Reset state and close only tabs created by this session",{session})
  tool("session_delete","Delete a named session and close its own tabs",{session:z.string()})
  tool("page_new","Open an HTTP(S) URL in a new driver-owned browser tab",{session,url:z.string().url()})
  tool("snapshot","Get compact visible controls with stable element references",{session})
  tool("execute","Run trusted local JavaScript with a real Playwright page, state, snapshot and ref helpers",{session,code:z.string(),timeoutMs:z.number().int().min(1000).max(120000).optional()})
  tool("screenshot","Capture the selected page as a PNG",{session,fullPage:z.boolean().optional()})
  tool("network_start","Start bounded request/response metadata capture without query strings, headers or bodies",{session})
  tool("network_status","Read bounded network summaries",{session})
  tool("network_stop","Stop network capture and return summaries",{session})
  tool("download","Start a download with the browser download manager",{session,url:z.string().url(),filename:z.string().optional()})
  tool("recording_start","Start bounded screenshot-sampled video; requires ffmpeg, no audio",{session,fps:z.number().min(1).max(15).optional()})
  tool("recording_status","Get recorder frame counts, dropped frames and errors",{session})
  tool("recording_stop","Stop capture and encode WebM under Neoism's local recordings directory",{session})
  tool("recording_cancel","Stop capture and discard buffered frames",{session})
  tool("flight_recorder_start","Start a bounded rolling screenshot buffer; requires ffmpeg",{session,fps:z.number().min(1).max(15).optional(),seconds:z.number().min(1).max(120).optional()})
  tool("flight_recorder_status","Inspect the rolling recorder",{session})
  tool("flight_recorder_save_last","Encode the last N seconds without stopping the rolling capture",{session,seconds:z.number().min(1).max(120).optional()})
  tool("flight_recorder_cancel","Stop and discard the rolling capture",{session})
  tool("handoff","Wait for the user to continue from Neoism's status panel in this tab",{session,message:z.string().optional(),timeoutMs:z.number().int().min(1000).max(600000).optional()})
  await server.connect(new StdioServerTransport())
}
if(process.env.NEOISM_BROWSER_ENTRY !== "firefox") void main().catch(error=>{process.stderr.write(`${shortError(error)}\n`);process.exitCode=1})
