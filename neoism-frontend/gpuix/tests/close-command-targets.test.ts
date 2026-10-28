import {expect,test} from "bun:test";
import {CommandController,type CommandOptions} from "../src/commands/CommandController";

test("exact palette q and qall aliases close their tab/workspace on every surface before fuzzy Quit",()=>{
 const surfaces:CommandOptions["surface"][]=["editor","markdown","terminal","agent","notebook","epub"];
 for(const surface of surfaces){
  const palette=new CommandController({surface,supportedActions:["Quit"]});
  try{
   for(const [command,action] of [["q","CloseTab"],["quit","CloseTab"],["quite","CloseTab"],["close","CloseTab"],["qa","CloseWorkspace"],["qall","CloseWorkspace"],["quitall","CloseWorkspace"]]){
    for(const prefix of ["",":"])for(const bang of ["","!"]){
     palette.query(prefix+command+bang);
     expect(palette.target()).toEqual({action,argument:bang?"discard":""});
     expect(CommandController.closeTarget(prefix+command+bang)).toEqual(palette.target());
     expect(palette.frame.rows).toHaveLength(1);expect(palette.frame.rows[0]?.action).toBe(action);
     expect(palette.frame.rows[0]?.displayTitle).not.toContain("Quit");
     expect(palette.frame.listRect[3]).toBeGreaterThanOrEqual(palette.frame.rowHeight);
    }
   }
   palette.query("qalligator");expect(palette.target()?.action).not.toBe("CloseWorkspace");
  }finally{palette.dispose();}
 }
});
