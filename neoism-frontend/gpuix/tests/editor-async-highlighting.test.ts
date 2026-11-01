import {expect,test} from 'bun:test';
import {NativeEditor} from '../src/native-core';

test('cold editor highlighting converges to whole-file multiline syntax without editing the document',async()=>{
 const text='/* comment begins\nstill in comment\n*/\nint value = 42;';
 const editor=new NativeEditor('async-highlighting.cpp',text,true),revision=editor.frame.revision;
 try{
  const deadline=Date.now()+5000;
  while(editor.frame.highlightAnimating){if(Date.now()>deadline)throw Error('Background highlighting did not settle');await Bun.sleep(10);editor.call('frame');}
  expect(editor.frame.lines.find(row=>row.line===1)?.runs.some(run=>run.token==='Comment'&&run.text.includes('still in comment'))).toBe(true);
  expect(editor.frame.revision).toBe(revision);expect(editor.text()).toBe(text);
  expect(editor.frame.highlightAnimating).toBe(false);
 }finally{editor.dispose();}
});
