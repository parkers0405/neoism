import { expect, test } from "bun:test";
import { coreCall } from "../src/native-core";
import type { PickerFrame } from "../src/ui/InlinePicker";
const geometry = { anchor: [100, 700, 600, 120], scale: 1 };
test("session directory headers paint folder icons above inset chats and stay grouped when searching",()=>{
 const options=[{title:"/project/a",value:"",header:true,section:"directory"},{title:"Alpha chat",value:"a",section:"/project/a",description:"/project/a"},{title:"/project/b",value:"",header:true,section:"directory"},{title:"Beta chat",value:"b",section:"/project/b",description:"/project/b"}];
 const {id}=coreCall<{id:number}>("picker.new",{kind:"sessions",title:"Sessions",options});
 try{
  const commands=coreCall<{commands:{kind:string;text?:string;x:number;y:number}[]}>("picker.paint",{id,...geometry,theme:"pastel_dark"}).commands.filter(command=>command.kind==="text");
  const folder=commands.find(command=>command.text==="/project/a")!,chat=commands.find(command=>command.text==="Alpha chat")!;
  expect(folder).toBeDefined();expect(chat.x-folder.x).toBeCloseTo(20,3);
  expect(commands.some(command=>command.x<folder.x&&Math.abs(command.y-folder.y)<5&&command.text!=="/project/a")).toBe(true);
  expect(coreCall<PickerFrame>("picker.query",{id,...geometry,query:"Beta"}).rows.map(row=>row.title)).toEqual(["/project/b","Beta chat"]);
 }finally{coreCall("picker.dispose",{id});}
});
test("model picker preserves headers, selection, filtering, and refreshed catalogs", () => {
  const options = [
    { title: "OpenAI", value: "openai", header: true },
    { title: "GPT", description: "OpenAI", value: "openai/gpt" },
    { title: "Anthropic", value: "anthropic", header: true },
    { title: "Opus", description: "Anthropic", value: "anthropic/opus" },
  ];
  const { id } = coreCall<{ id: number }>("picker.new", { kind: "model", title: "Select model", options });
  const call = (op: string, args: Record<string, unknown> = {}) => coreCall<PickerFrame>(`picker.${op}`, { id, ...geometry, ...args });
  try {
    expect(call("frame").selectedValue).toBe("openai/gpt");
    expect(call("move", { delta: 1 }).selectedValue).toBe("anthropic/opus");
    expect(call("query", { query: "opus" }).rows.filter((o) => !o.header).map((o) => o.value)).toEqual(["anthropic/opus"]);
    expect(call("options", { options: [...options, { title: "Opus 2", value: "anthropic/opus2" }] }).rows.filter((o) => !o.header)).toHaveLength(2);
    const frame = call("frame"); expect(frame.selectedValue).toBe("anthropic/opus");
    expect(frame.rect[0]).toBe(100); expect(frame.rect[2]).toBe(600);
  } finally { coreCall("picker.dispose", { id }); }
});
test("picker wheel and keyboard selection retain the native scroll and cursor springs", () => {
  const { id } = coreCall<{ id: number }>("picker.new", { kind: "agent", title: "Agents", options: Array.from({ length: 40 }, (_, n) => ({ title: `Agent ${n}`, value: String(n) })) });
  try {
    const frame = coreCall<PickerFrame>("picker.scroll", { id, ...geometry, delta: -100 });
    expect(frame.animating).toBe(true); expect(frame.offset).toBeLessThanOrEqual(0);
    const moved = coreCall<PickerFrame>("picker.move", { id, ...geometry, delta: 10 });
    expect(moved.selected).toBeGreaterThan(0); expect(moved.animating).toBe(true);
    expect(moved.limit).toBeLessThanOrEqual(8);
  } finally { coreCall("picker.dispose", { id }); }
});
test("question policy commits sequential option/custom answers and permits retrying the same answer", () => {
  const { id } = coreCall<{ id: number }>("question.new", { request: { id: "question-1", sessionID: "thread-1", questions: [
    { question: "Color?", options: [{ label: "Red", description: "Warm" }, { label: "Blue", description: "Cool" }] },
    { question: "When?", options: [] },
  ] } });
  const call = (op: string, args: Record<string, unknown> = {}) => coreCall<PickerFrame>(`question.${op}`, { id, ...geometry, ...args });
  try {
    expect(call("frame").title).toBe("(1/2) Color?");
    expect(call("query", { query: "Blue" }).rows).toHaveLength(2);
    expect(call("commit").title).toBe("(2/2) When?");
    call("query", { query: "Tomorrow" });
    const final = call("commit"); expect(final.answers).toEqual([["Blue"], ["Tomorrow"]]); expect(final.responding).toBe(true);
    expect(call("commit").answers).toBeUndefined();
    call("retry"); expect(call("commit").answers).toEqual([["Blue"], ["Tomorrow"]]);
  } finally { coreCall("question.dispose", { id }); }
});
test("attachment tokens use the desktop MIME and collision policies", () => {
  expect(coreCall<{ token: string }>("attachment.token", { filename: "photo.png", mime: "image/png", index: 1 }).token).toBe("[image1]");
  expect(coreCall<{ token: string }>("attachment.token", { filename: "paper.pdf", mime: "application/pdf", index: 2 }).token).toBe("[pdf2]");
  const reply = coreCall<{ token: string }>("attachment.token", { filename: "notes.txt", mime: "text/plain", index: 1, input: "[file1: notes.txt]", tokens: [] });
  expect(reply.token).not.toBe("[file1: notes.txt]"); expect(reply.token).toContain("notes.txt");
});

test("auth entry keeps source form geometry stable while committing its actual typed value",()=>{
 const {id}=coreCall<{id:number}>("question.new",{request:{id:"auth-entry",sessionID:"one",questions:[{question:"API key",options:[]}]}});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<PickerFrame>(`question.${op}`,{id,...geometry,entry:true,...args});
 try{
  const before=call("frame"),typed=call("query",{query:"fixture-secret-only"});
  expect(typed.rect).toEqual(before.rect);
  expect(call("commit").answers).toEqual([["fixture-secret-only"]]);
 }finally{coreCall("question.dispose",{id});}
});

test("session pages keep native scroll, selection and query without mounting the entire catalog",async()=>{
 const options=[{title:"Pinned",value:"",header:true},{title:"Pinned chat",value:"pin",pinned:true},{title:"Recent",value:"",header:true},...Array.from({length:50},(_,n)=>({title:`Chat ${n}`,value:String(n),current:n===0}))];
 const{id}=coreCall<{id:number}>("picker.new",{kind:"sessions",title:"Sessions",options});
 const call=(op:string,args:Record<string,unknown>={})=>coreCall<PickerFrame>(`picker.${op}`,{id,...geometry,...args});
 try{
  const first=call("frame");expect(first.rows[1].pinned).toBe(true);expect(first.rows[3].current).toBe(true);expect(first.selectedValue).toBe("pin");
  expect(call("move",{delta:1}).selectedValue).toBe("0");
  call("scroll",{delta:-2000});for(let n=0;n<15;n++){await Bun.sleep(30);call("frame");}
  const tail=call("frame");expect(tail.top).toBeGreaterThan(0);
  const appended=call("options",{options:[...options,...Array.from({length:50},(_,n)=>({title:`Chat ${n+50}`,value:String(n+50)}))]});
  expect(appended.top).toBe(tail.top);expect(appended.selectedValue).toBe(tail.selectedValue);expect(appended.rows.length).toBeLessThanOrEqual(appended.limit+3);
  call("query",{query:"Chat 99"});const filtered=call("options",{options:[...options,{title:"Chat 99",value:"99"}]});
  expect(filtered.query).toBe("Chat 99");expect(filtered.rows.filter(row=>!row.header).map(row=>row.value)).toEqual(["99"]);
 }finally{coreCall("picker.dispose",{id});}
});
