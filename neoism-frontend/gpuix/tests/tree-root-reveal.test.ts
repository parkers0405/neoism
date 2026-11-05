import { expect, test } from "bun:test";
import { TreeRootReveal, type TreeRevealInput, type TreeRevealIdentity } from "../src/ui/tree-root-reveal";

const initial: TreeRevealInput = {key:'a:/repo',ready:true,total:50000,height:100,rowHeight:20,top:25000,offset:0,reducedMotion:false,active:true};
function fixture(identity: TreeRevealIdentity = {}) {
  let now = 0, next = 0, changes = 0;
  const timers = new Map<number, () => void>();
  const reveal = new TreeRootReveal(identity, () => changes++, () => now, callback => {
    const id = ++next; timers.set(id, callback); return () => {timers.delete(id);};
  });
  return {reveal,timers,identity,get changes(){return changes;},
    tick(time:number){now=time;const callbacks=[...timers.values()];timers.clear();for(const callback of callbacks)callback();}};
}

test('legacy cubic sweep uses viewport rank, fades ink and adds positive scaled 12px',()=>{
  const f=fixture();f.reveal.update(initial);
  expect(f.reveal.row(25000,2)).toEqual({opacity:0,offsetX:24});
  f.tick(160);
  expect(f.reveal.row(25000,2)).toEqual({opacity:.875,offsetX:3});
  expect(f.reveal.row(25001).opacity).toBeCloseTo(1-(1-148/320)**3);
  expect(f.timers.size).toBe(1);
  f.tick(380);expect(f.reveal.row(25004)).toEqual({opacity:1,offsetX:0});expect(f.timers.size).toBe(0);
});

test('normal refresh and scrolling do not restart; partially visible first row owns beat zero',()=>{
  const f=fixture();f.reveal.update(initial);f.tick(160);
  f.reveal.update({...initial,top:25010,offset:-1,total:50001});
  expect(f.reveal.row(25010).opacity).toBe(.875);
  expect(f.reveal.row(25011).opacity).toBeCloseTo(1-(1-148/320)**3);
  f.tick(1000);f.reveal.update(initial);expect(f.timers.size).toBe(0);
});

test('async root listing arms without ticking or fading skeleton; empty result consumes sweep',()=>{
  const f=fixture();f.reveal.update({...initial,ready:false,total:0});f.tick(5000);
  expect(f.timers.size).toBe(0);expect(f.reveal.row(0).opacity).toBe(1);
  f.reveal.update(initial);expect(f.reveal.row(25000).opacity).toBe(0);
  f.reveal.update({...initial,key:'empty',total:0});expect(f.timers.size).toBe(0);
  f.reveal.update({...initial,key:'empty'});expect(f.reveal.row(25000).opacity).toBe(1);
});

test('rapid A/B/A activation cancels old frame; retained root still sweeps on workspace id change',()=>{
  const f=fixture();f.reveal.update(initial);f.tick(160);
  f.reveal.update({...initial,key:'b:/repo'});expect(f.timers.size).toBe(1);expect(f.reveal.row(25000).opacity).toBe(0);
  f.tick(200);expect(f.reveal.row(25000).opacity).toBeCloseTo(1-(1-40/320)**3);
  f.reveal.update(initial);expect(f.timers.size).toBe(1);expect(f.reveal.row(25000).opacity).toBe(0);
  f.reveal.dispose();expect(f.timers.size).toBe(0);
  const changes=f.changes;f.tick(10000);f.reveal.update(initial);expect(f.changes).toBe(changes);
});

test('reduced motion and backgrounding settle immediately, including a pending listing',()=>{
  for(const flag of ['reducedMotion','active'] as const){
    const f=fixture();f.reveal.update(initial);f.tick(100);
    const disabled={...initial,[flag]:flag==='reducedMotion'};
    f.reveal.update(disabled);expect(f.timers.size).toBe(0);expect(f.reveal.row(25000)).toEqual({opacity:1,offsetX:0});
    f.reveal.update(initial);expect(f.timers.size).toBe(0);
    f.reveal.update({...disabled,key:'other',ready:false});f.reveal.update({...initial,key:'other'});expect(f.timers.size).toBe(0);
  }
});

test('window identities are isolated and generation remount does not replay or own orphan frames',()=>{
  const one=fixture(),two=fixture();one.reveal.update(initial);two.reveal.update(initial);
  one.reveal.dispose();expect(one.timers.size).toBe(0);expect(two.timers.size).toBe(1);
  const remount=fixture(one.identity);remount.reveal.update(initial);expect(remount.timers.size).toBe(0);
  remount.reveal.update({...initial,key:'different-root'});expect(remount.timers.size).toBe(1);
  two.tick(1000);expect(two.timers.size).toBe(0);
});

test('listing before physical layout waits for a usable viewport without burning its sweep',()=>{
  const f=fixture();f.reveal.update({...initial,height:0});f.tick(1000);
  expect(f.timers.size).toBe(0);
  f.reveal.update(initial);expect(f.reveal.row(25000).opacity).toBe(0);expect(f.timers.size).toBe(1);
});

test('an unpresented superseded generation does not consume the activation identity',()=>{
  const identity={key:'old'},hidden=fixture(identity);
  hidden.reveal.update({...initial,ready:false});hidden.tick(5000);
  expect(identity.key).toBe('old');expect(hidden.timers.size).toBe(0);
  hidden.reveal.dispose();
  const replacement=fixture(identity);replacement.reveal.update({...initial,ready:false});replacement.tick(5000);
  replacement.reveal.update(initial);expect(replacement.reveal.row(25000).opacity).toBe(0);
  expect(identity.key).toBe(initial.key);replacement.reveal.dispose();
  const configOnly=fixture(identity);configOnly.reveal.update(initial);
  expect(configOnly.reveal.row(25000).opacity).toBe(1);expect(configOnly.timers.size).toBe(0);
});