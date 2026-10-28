import {expect,test} from "bun:test";
import {fileURLToPath} from "node:url";
import {verticalTabScroll} from "../src/layout/tab-scroll";
import {hoverTitleFrame} from "../src/layout/HoverTabTitle";
test("tab scrolling respects dominant axis, exact original notch step and clamped overflow",()=>{
 expect(verticalTabScroll({deltaY:-1},0,300,1000)).toBe(-60);expect(verticalTabScroll({deltaY:-1},0,1000,300)).toBe(0);expect(verticalTabScroll({deltaX:12,deltaY:3,precise:true},-40,300,1000)).toBeUndefined();expect(verticalTabScroll({deltaX:2,deltaY:-10,precise:true},-40,300,1000)).toBe(-50);expect(verticalTabScroll({deltaY:.1,precise:true},-40,300,1000)).toBeUndefined();
 expect(hoverTitleFrame(.2,70,1).full).toBe(false);expect(hoverTitleFrame(1.6,70,1).offset).toBeCloseTo(35);expect(hoverTitleFrame(3.6,70,1).offset).toBeCloseTo(35);expect(hoverTitleFrame(1.6,140,2).offset).toBeCloseTo(70);
});
test("actual Solid workspace and buffer rows scroll and reveal hovered titles without remounting",async()=>{
 const child=Bun.spawn([process.execPath,"run","tests/support/tab-scroll-title-check.tsx"],{cwd:fileURLToPath(new URL("..",import.meta.url)),stdout:"pipe",stderr:"pipe"});const[status,stdout,stderr]=await Promise.all([child.exited,new Response(child.stdout).text(),new Response(child.stderr).text()]);expect({status,stderr}).toEqual({status:0,stderr:""});expect(JSON.parse(stdout)).toEqual({noWindow:true,actualBothTabRows:true,verticalWheelAndPrecise:true,horizontalNotDoubled:true,naturalDirection:true,overflowClamped:true,stableTabAndBodyIds:true,fullTitleMarquee:true,canonicalTiming:true,fixedCloseGeometry:true,idleStops:true});
},10000);
