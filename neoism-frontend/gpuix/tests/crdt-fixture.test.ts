import {expect,test} from "bun:test";
import {createCrdtFixture} from "./support/crdt-fixture";

test("document fixture preserves opaque Unix, drive and UNC host IDs on every runner",()=>{
 for(const path of ["/project/a/literal #100%.ts","C:\\project\\a\\source.ts","\\\\host\\share\\source.ts"]){
  const reads:string[]=[],writes:Array<[string,string]>=[],fixture=createCrdtFixture(value=>{reads.push(value);return"fixture source";},(path,text)=>writes.push([path,text]));
  const buffer_id=`file://${path}`,replies:any[]=[];
  try{
   fixture.handle({AcquireBuffer:{buffer_id,owner_id:"test-owner",initial_text:""}},reply=>replies.push(reply));
   fixture.handle({Owned:{owner_id:"test-owner",message:{SaveBuffer:{buffer_id}}}},reply=>replies.push(reply));
   expect(reads).toEqual([path]);expect(writes).toEqual([[path,"fixture source"]]);
   expect(replies[0].Snapshot.buffer_id).toBe(buffer_id);expect(replies[1].Saved).toEqual({buffer_id,bytes_written:14});
  }finally{fixture.dispose();}
 }
});