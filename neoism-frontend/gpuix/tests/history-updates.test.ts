import {test,expect} from 'bun:test';
import {HistoryUpdates} from '../src/agent/history-updates';
test('stream batches index retained history and publish without mutating older snapshots',()=>{
 const updates=new HistoryUpdates<{id:string;text:string}>(),base=Array.from({length:10000},(_,i)=>({id:String(i),text:'old'}));const history={session:base};
 for(let i=0;i<1000;i++){const old=updates.get('session',base,'9999')!;updates.stage('session',base,{...old,text:old.text+'x'});}
 expect(base[9999].text).toBe('old');const next=updates.flush(history);expect(next.session).not.toBe(base);expect(next.session[0]).toBe(base[0]);expect(next.session[9999].text).toBe('old'+'x'.repeat(1000));expect(updates.flush(next)).toBe(next);
 updates.stage('session',next.session,{id:'new',text:'one'});updates.stage('session',next.session,{id:'new',text:'two'});const appended=updates.flush(next);expect(appended.session).toHaveLength(10001);expect(appended.session.at(-1)?.text).toBe('two');
});
test('replacement history invalidates old indexes and stale pending state',()=>{
 const updates=new HistoryUpdates<{id:string}>(),history={s:[{id:'a'}]};updates.stage('s',history.s,{id:'b'});const replacement={s:[{id:'c'}]};expect(updates.flush(replacement).s).toBe(replacement.s);expect(updates.get('s',replacement.s,'a')).toBeUndefined();expect(updates.get('s',replacement.s,'c')).toBe(replacement.s[0]);
});
test('legacy duplicate IDs retain first-match update semantics',()=>{
 const updates=new HistoryUpdates<{id:string;text:string}>(),history={s:[{id:'same',text:'first'},{id:'same',text:'second'}]};
 expect(updates.get('s',history.s,'same')?.text).toBe('first');updates.stage('s',history.s,{id:'same',text:'updated'});expect(updates.flush(history).s.map(row=>row.text)).toEqual(['updated','second']);
});
