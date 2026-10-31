import assert from "node:assert/strict";
import {createRoot} from "@gpuix/solid";
import {App} from "../../src/ui/App";
import {AppVisualState} from "../../src/ui/AppVisualState";
import {loadConfig} from "../../src/config";
import {DaemonConnection} from "../../src/daemon/client";
import {WorkspaceModel} from "../../src/model";
import {createAgentDaemon} from "./agent-daemon";
import {createRecordingAppRenderer} from "./full-app-recording-renderer";
import {usePresentationScheduler} from "../../src/animation/frame-scheduler";
const daemon=createAgentDaemon(),config={...loadConfig(["--config","/nonexistent/sidebar-focus"],{}),endpoint:daemon.endpoint,reducedMotion:true,cursorBlink:false};
const client=new DaemonConnection(config),model=new WorkspaceModel(client,config),visuals=new AppVisualState(),host=createRecordingAppRenderer(),root=createRoot(host.renderer);
const callbacks=new Set<()=>void>(),restore=usePresentationScheduler(callback=>{callbacks.add(callback);return()=>callbacks.delete(callback);});
const pulse=async()=>{const list=[...callbacks];callbacks.clear();for(const frame of list)frame();await host.pump(root);await Bun.sleep(5);};
let stage="start";
const until=async(check:()=>boolean)=>{const end=Date.now()+4000;while(!check()){await pulse();if(Date.now()>end)throw Error("Agent sidebar focus did not settle: "+JSON.stringify({stage,focused:host.focused,tree:host.find("file-tree")?.id,composer:host.find("composer")?.id,ui:model.state.ui}));}};
const key=async(key:string,alt=false)=>{host.dispatchKey(root,host.focused,{key,modifiers:{alt,ctrl:false,cmd:false,shift:false}});await pulse();};
try {
 client.start();await until(()=>client.state.phase==="connected");
 model.update({activeWorkspace:{id:"a",host_id:"host",title:"A",root_dir:"/project/a"},workspaces:[{id:"a",host_id:"host",title:"A",root_dir:"/project/a"}],loading:false,threads:daemon.threads,history:{one:[{id:"answer",role:"Assistant",kind:"Assistant",text:"A saved conversation with selectable text.",usage:{input:100,output:20,context_limit:1000}}]}});
 stage="load history";model.openThread(daemon.threads[0]);

 stage="render";root.render(()=><App model={model} config={config} visuals={visuals}/>);await host.pump(root);
 await until(()=>!!model.state.history.one?.length);
 stage="saved usage";if(!host.find("agent-details"))await key("h",true);
 await until(()=>!!host.find("context-meter"));
 assert.ok([...host.nodes.values()].some(node=>node.text==="120 / 1000 tokens"),"Saved conversation usage must populate the right panel before any new response");
 stage="open tree";if(!host.find("file-tree"))await key("e",true);await until(()=>!!host.find("file-tree")&&!!host.find("conversation"));
 stage="focus tree";await key("left",true);await until(()=>host.focused===host.find("file-tree")?.id);
 const tree=host.focused,conversation=host.find("conversation")!;
 assert.equal(conversation.props["data-preserve-keyboard-focus"],true,"Native default mouse focus must be suppressed while the sidebar owns navigation");
 root.dispatch({elementId:conversation.id,eventType:"mouseDown",button:0,x:400,y:200});
 root.dispatch({elementId:conversation.id,eventType:"mouseMove",x:500,y:200});
 root.dispatch({elementId:conversation.id,eventType:"mouseUp",button:0,x:500,y:200});
 await pulse();assert.equal(host.focused,tree,"Selecting or clicking transcript content must preserve sidebar keyboard ownership");
 await key("down");assert.equal(host.focused,tree);
 // Explicit keyboard movement still transfers to the composer normally.
 stage="focus composer";await key("right",true);await until(()=>host.focused===host.find("composer")?.id);
 assert.equal(conversation.props["data-preserve-keyboard-focus"],false);
 console.log(JSON.stringify({actualApp:true,sidebarSurvivesTranscriptSelection:true,explicitKeyboardTransfer:true}));
} finally {root.unmount();restore();visuals.dispose();model.dispose();client.stop();daemon.stop();}
