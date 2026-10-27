import http from "node:http"
import { timingSafeEqual } from "node:crypto"
import { WebSocketServer } from "ws"
import { ExtensionBridge, pairingToken } from "./bridge.ts"
import { Driver } from "./driver.ts"
import { enginePort, shortError, type Engine } from "./protocol.ts"

export async function startServer(engine:Engine,port=enginePort(engine)) {
  const token=await pairingToken(engine), bridge=new ExtensionBridge(engine,token), driver=new Driver(bridge)
  const sockets=new Set<import("node:net").Socket>()
  const server=http.createServer((request,response)=>{ void (async()=> {
    const host = request.headers.host ?? ""
    if (!/^127\.0\.0\.1:\d+$/.test(host) || request.headers.origin) {response.writeHead(403);response.end();return}
    response.setHeader("Content-Type","application/json")
    if (request.method === "GET" && request.url === "/health") {response.end(JSON.stringify({service:"neoism-browser",version:1,engine}));return}
    const auth=request.headers.authorization ?? "", expected=`Bearer ${token}`
    if (auth.length !== expected.length || !timingSafeEqual(Buffer.from(auth),Buffer.from(expected))) {response.writeHead(401);response.end(JSON.stringify({error:"Local authentication required"}));return}
    if(request.method !== "POST" || request.url !== "/rpc") {response.writeHead(404);response.end();return}
    let body=""
    for await (const chunk of request) {body+=chunk;if(Buffer.byteLength(body)>1024*1024){response.writeHead(413);response.end();return}}
    const message=JSON.parse(body)
    if(typeof message.method !== "string" || (message.args !== undefined && (!message.args || typeof message.args !== "object" || Array.isArray(message.args)))) throw new Error("Invalid Neoism browser request")
    try {
      const result=await driver.invoke(message.method,message.args ?? {})
      const encoded=JSON.stringify({result})
      if (encoded.length>12*1024*1024) throw new Error("Browser result exceeds 12MB; use a smaller result or screenshot")
      response.end(encoded)
    } catch(error) {response.end(JSON.stringify({error:shortError(error)}))}
  })().catch(error=>{if(!response.headersSent)response.writeHead(400);response.end(JSON.stringify({error:shortError(error)}))}) })
  const websocket=new WebSocketServer({noServer:true,maxPayload:16*1024*1024})
  server.on("upgrade",(request,socket,head)=>{
    const origin=request.headers.origin
    if (request.url !== "/bridge" || !/^127\.0\.0\.1:\d+$/.test(request.headers.host ?? "") || (origin && !/^(chrome|moz)-extension:\/\/[^/]+$/.test(origin))) {socket.destroy();return}
    websocket.handleUpgrade(request,socket,head,ws=>bridge.accept(ws))
  })
  server.on("connection",socket=>{sockets.add(socket);socket.on("close",()=>sockets.delete(socket))})
  await new Promise<void>((resolve,reject)=>{server.once("error",reject);server.listen(port,"127.0.0.1",()=>{server.off("error",reject);resolve()})})
  return {driver,bridge,port:(server.address() as import("node:net").AddressInfo).port,token,async close(){await driver.close();for(const ws of websocket.clients)ws.terminate();websocket.close();for(const socket of sockets)socket.destroy();await new Promise<void>(resolve=>server.close(()=>resolve()))}}
}
