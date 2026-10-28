import { launchNative } from "../tests/support/native-launch";
const app=await launchNative({command:process.execPath,args:["tests/support/provider-window.tsx"],cwd:process.cwd()});
const json=async(args:string[])=>JSON.parse(await new Response(Bun.spawn(args,{stdout:"pipe"}).stdout).text());
try {
 const logo=app.getByTestId("composer-provider-logo"),skeleton=app.getByTestId("loading-skeleton:picker");
 await logo.waitFor({timeoutMs:15000});await skeleton.waitFor();await Bun.sleep(350);
 const bounds=await logo.bounds(),skeletonBounds=await skeleton.bounds();
 if(bounds.width<50||bounds.height<50||skeletonBounds.width<200||skeletonBounds.height<100)throw new Error("Logo/skeleton never acquired positive native paint bounds");
 const clients=await json(["hyprctl","clients","-j"]),client=clients.find((c:any)=>c.title==="Neoism provider raster verification");
 if(!client)throw new Error("Cannot locate the controlled native window for pixel proof");
 const monitors=await json(["hyprctl","monitors","-j"]),monitor=monitors.find((m:any)=>m.id===client.monitor);
 const capture=async(path:string)=>{const child=Bun.spawn(["grim","-o",monitor.name,path],{stderr:"inherit"});if(await child.exited)throw new Error("Compositor capture failed");};
 const first="/tmp/neoism-provider-proof-a.png",second="/tmp/neoism-provider-proof-b.png";
 await capture(first);await Bun.sleep(350);await capture(second);
 const verify=Bun.spawn(["/usr/bin/python3","-c",`from PIL import Image\nimport sys,json\na,b=map(Image.open,sys.argv[1:3]);c,m,r,s=map(json.loads,sys.argv[3:]);scale=m['scale']\ndef crop(image,rect):\n x=(c['at'][0]-m['x']+rect['x'])*scale;y=(c['at'][1]-m['y']+rect['y'])*scale\n return image.crop((round(x),round(y),round(x+rect['width']*scale),round(y+rect['height']*scale))).convert('RGB')\npixels=list(crop(a,r).getdata());paint=sum(abs(p[0]-217)<4 and abs(p[1]-119)<4 and abs(p[2]-87)<4 for p in pixels)\nassert 30<paint<len(pixels)*0.9,('Original provider mask/tint did not paint its silhouette',paint)\nx=list(crop(a,s).getdata());y=list(crop(b,s).getdata());change=sum(p!=q for p,q in zip(x,y));gray=sum(3<p[0]<30 and p[0]==p[1]==p[2] for p in x)\nassert gray>100,('Theme-muted skeleton did not paint',gray)\nassert change>100,('Skeleton did not animate',change)\nprint('Original atlas alpha mask and brand tint painted:',paint,'pixels; animated muted skeleton:',gray,'pixels,',change,'changed.')`,first,second,JSON.stringify(client),JSON.stringify(monitor),JSON.stringify(bounds),JSON.stringify(skeletonBounds)],{stdout:"inherit",stderr:"inherit"});
 if(await verify.exited)throw new Error("Native logo/skeleton pixel proof failed");
} finally {await app.close();}
