import {expect,test} from "bun:test";
import {serverDraftValues,serverFormOperation,type ServerDraft} from "../src/menu-actions/server-actions";
const draft:ServerDraft={mode:"host",fast:false,directory:"/project",address:"",name:"My team",password:""};
test("original Host/Join form switches retain each target and preserve exact passwords",()=>{
 const next=serverDraftValues(draft,[["create_dir","/project/edited"],["name","Team"],["token","  secret with spaces  "]]);
 expect(next).toMatchObject({directory:"/project/edited",name:"Team",password:"  secret with spaces  "});
 expect(serverFormOperation(next)).toEqual({action:"host",args:{directory:"/project/edited",name:"Team",password:"  secret with spaces  "}});
 const joined=serverDraftValues({...next,mode:"join",fast:true},[["fast_join_address","https://invite.example/#neoism-fast=abc"]]);
 expect(joined.directory).toBe("/project/edited");expect(serverFormOperation(joined).action).toBe("join-fast");
 expect(serverFormOperation({...next,mode:"join",serverId:"saved",address:"ws://host/session"}).action).toBe("save");
});
test("incomplete Fast host and join forms cannot create a server",()=>{
 expect(()=>serverFormOperation({...draft,fast:true})).toThrow("Password is required");
 expect(()=>serverFormOperation({...draft,directory:" "})).toThrow("Project folder is required");
 expect(()=>serverFormOperation({...draft,mode:"join"})).toThrow("Server address is required");
});
