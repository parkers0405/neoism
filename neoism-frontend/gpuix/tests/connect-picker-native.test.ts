import {expect,test} from "bun:test";
import {coreCall} from "../src/native-core";
import {connectCatalog} from "../src/agent/connect-catalog";
import type {PickerFrame,PickerOption} from "../src/ui/InlinePicker";
const geometry={anchor:[100,700,600,120],scale:1};
test("Connect strips model catalogs before FFI without changing provider choices",()=>{
 const catalog={all:[{id:"openai",name:"OpenAI",models:{large:{description:"unused".repeat(10000)}},options:{unused:true}},{id:"custom",name:"",models:{},options:{}}],connected:["openai"]};
 const compact=connectCatalog(catalog);
 expect(JSON.stringify(compact).length).toBeLessThan(200);
 expect(coreCall("picker.connect-options",{catalog:compact})).toEqual(coreCall("picker.connect-options",{catalog}));
 const provider={id:"openai",name:"OpenAI",get models():never{throw new Error("Picker must not traverse models");},get options():never{throw new Error("Picker must not traverse options");}};
 expect(connectCatalog({all:[provider],connected:["openai"]})).toEqual({all:[{id:"openai",name:"OpenAI"}],connected:["openai"]});
});
test("Connect provider rows use canonical popular ordering, connected marks and alphabetical remaining providers",()=>{
 const rows=coreCall<PickerOption[]>("picker.connect-options",{catalog:{all:[{id:"z",name:"Zulu"},{id:"openai",name:"OpenAI"},{id:"github-copilot",name:"Copilot"},{id:"anthropic",name:"Anthropic"},{id:"a",name:"alpha"},{id:"openrouter",name:"OpenRouter"},{id:"claude-code",name:"Claude Code"},{id:"",name:"invalid"}],connected:["openai","a"]}});
 expect(rows.map(row=>row.title)).toEqual(["Popular","Claude Code","Anthropic","✓ OpenAI","OpenRouter","Copilot","Providers","✓ alpha","Zulu"]);
 expect(rows.filter(row=>row.header).map(row=>row.title)).toEqual(["Popular","Providers"]);
 expect(rows.find(row=>row.value==="openai")?.footer).toBe("connected");
 const{id}=coreCall<{id:number}>("picker.new",{kind:"connect",title:"Connect a provider",options:rows});
 try{
  expect(coreCall<PickerFrame>("picker.frame",{id,...geometry}).selectedValue).toBe("claude-code");
  const filtered=coreCall<PickerFrame>("picker.query",{id,...geometry,query:"alpha"});
  expect(filtered.rows.map(row=>row.title)).toEqual(["Providers","✓ alpha"]);expect(filtered.selectedValue).toBe("a");
 }finally{coreCall("picker.dispose",{id});}
 const empty=coreCall<PickerOption[]>("picker.connect-options",{catalog:{all:[],connected:[]}});expect(empty[0].title).toBe("No providers available");expect(empty[0].value).toBe("");
});
test("Connect account stages retain canonical account headers and safe Cancel selection",()=>{
 for(const kind of ["connect-account","connect-account-actions","connect-confirm"]){
  const options=kind==="connect-account"?[{title:"Add account",value:"__add_account__"},{title:"Connected accounts",value:"",header:true},{title:"Personal",description:"oauth",footer:"default",value:"account-one"}]:kind==="connect-confirm"?[{title:"Disconnect",value:"confirm_disconnect"},{title:"Cancel",value:"cancel"}]:[{title:"Select account",value:"select"},{title:"Rename",value:"rename"},{title:"Disconnect",value:"disconnect"}];
  const{id}=coreCall<{id:number}>("picker.new",{kind,title:"Account",options,selected:kind==="connect-confirm"?1:0});
  try{
   const frame=coreCall<PickerFrame>("picker.frame",{id,...geometry});expect(frame.selectedValue).toBe(kind==="connect-confirm"?"cancel":options[0].value);
   if(kind==="connect-account")expect(coreCall<PickerFrame>("picker.move",{id,...geometry,delta:1}).selectedValue).toBe("account-one");
  }finally{coreCall("picker.dispose",{id});}
 }
});

test("Native Connect label and secret entry preserve original supplied placeholders",()=>{
 for(const kind of ["input","secret"]){
  const{id}=coreCall<{id:number}>("picker.new",{kind,title:"Account entry",options:[],placeholder:kind==="input"?"Account label":"API key"});
  try{
   const paint=()=>coreCall<{commands:{kind:string;text?:string}[]}>("picker.paint",{id,...geometry,theme:"pastel_dark"}).commands.filter(command=>command.kind==="text").map(command=>command.text).join(" ");
   expect(paint()).toContain(kind==="input"?"Account label":"API key");
   coreCall("picker.options",{id,...geometry,options:[],placeholder:"OAuth token"});expect(paint()).toContain("OAuth token");
  }finally{coreCall("picker.dispose",{id});}
 }
});

test("Native Connect secret entry paints masked bytes and no empty result row while preserving submit value",()=>{
 const{id}=coreCall<{id:number}>("picker.new",{kind:"secret",title:"Manually enter API Key",options:[],placeholder:"API key"});
 try{
  const typed=coreCall<PickerFrame>("picker.query",{id,...geometry,query:"fixture-key-ñ"});expect(typed.query).toBe("fixture-key-ñ");
  const painted=coreCall<{commands:{kind:string;text?:string}[];rect:number[]}>("picker.paint",{id,...geometry,theme:"pastel_dark"});
  const text=painted.commands.filter(command=>command.kind==="text").map(command=>command.text).join(" ");
  expect(text).toContain("•".repeat([...typed.query].length));expect(text).not.toContain("fixture-key");expect(text).not.toContain("No results");
  const label=coreCall<{id:number}>("picker.new",{kind:"input",title:"Label new account",options:[],placeholder:"Account label"});
  try{const entry=coreCall<{rect:number[]}>("picker.paint",{id:label.id,...geometry,theme:"pastel_dark"});expect(entry.rect[3]).toBe(painted.rect[3]);}finally{coreCall("picker.dispose",{id:label.id});}
 }finally{coreCall("picker.dispose",{id});}
});
