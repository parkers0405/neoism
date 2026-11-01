import {expect,test} from "bun:test";
import {PromptHistory} from "../src/agent/PromptHistory";
const draft=(text:string)=>({text,attachments:[]});
test("prompt history walks both directions and restores the unfinished draft with attachments",()=>{
 const history=new PromptHistory(),entries=[draft("first"),draft("second")],current={text:"unfinished [image1]",attachments:[{kind:"image/png",path:"shot.png",name:"shot.png",token:"[image1]",bytes:[1,2]}]};
 expect(history.previous(entries,current)).toEqual(entries[1]);expect(history.previous(entries,draft("second"))).toEqual(entries[0]);expect(history.previous(entries,draft("first"))).toEqual(entries[0]);
 expect(history.next()).toEqual(entries[1]);expect(history.next()).toEqual(current);expect(history.next()).toBeUndefined();
});
test("editing or switching composers resets navigation without recalling an obsolete draft",()=>{
 const history=new PromptHistory();history.previous([draft("sent")],draft("old draft"));history.reset();expect(history.next()).toBeUndefined();expect(history.previous([],draft("new"))).toBeUndefined();
});
