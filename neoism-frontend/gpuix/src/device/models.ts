/** Original Neoism models, with the same selectable device families as T3.
 * No third-party geometry, CAD, branding textures, or model files are used. */
export type DeviceModel = "iphone-18-pro" | "iphone-18-pro-max" | "iphone-duo" | "ipad-pro-13-m5";
export type DeviceDisplay = "inner" | "cover";
export type DeviceFinish = "burgundy" | "champagne" | "silver" | "graphite" | "blue" | "orange" | "gold";
export interface DeviceModelProfile {
  id:DeviceModel;label:string;shortLabel:string;description:string;
  foldable:boolean;keyboard:boolean;defaultFinish:DeviceFinish;finishes:readonly DeviceFinish[];
}
export const deviceFinishes:Record<DeviceFinish,{label:string;color:string;rgb:readonly[number,number,number]}>= {
  burgundy:{label:"Burgundy",color:"#432528",rgb:[.262,.144,.158]},
  champagne:{label:"White titanium",color:"#f2efe7",rgb:[.95,.936,.906]},
  silver:{label:"Silver",color:"#c5c8cd",rgb:[.773,.784,.804]},
  graphite:{label:"Graphite",color:"#343942",rgb:[.204,.224,.259]},
  blue:{label:"Deep blue",color:"#424f67",rgb:[.259,.310,.404]},
  orange:{label:"Copper orange",color:"#bf693e",rgb:[.749,.412,.243]},
  gold:{label:"Pale gold",color:"#c3af91",rgb:[.765,.686,.569]},
};
export const deviceModels:readonly DeviceModelProfile[]=[
  {id:"iphone-18-pro",label:"iPhone 18 Pro",shortLabel:"18 Pro",description:"Compact Pro / triple-camera shelf",foldable:false,keyboard:false,defaultFinish:"burgundy",finishes:["burgundy","silver","blue","orange"]},
  {id:"iphone-18-pro-max",label:"iPhone 18 Pro Max",shortLabel:"18 Pro Max",description:"Larger Pro body and display",foldable:false,keyboard:false,defaultFinish:"burgundy",finishes:["burgundy","blue","silver","orange"]},
  {id:"iphone-duo",label:"iPhone Duo",shortLabel:"Duo",description:"Clamshell inner display and cover screen",foldable:true,keyboard:false,defaultFinish:"champagne",finishes:["champagne","graphite","gold"]},
  {id:"ipad-pro-13-m5",label:"iPad Pro 13-inch (M5)",shortLabel:"iPad Pro 13",description:"Thin tablet / optional keyboard and stand",foldable:false,keyboard:true,defaultFinish:"graphite",finishes:["graphite","silver"]},
];
export function deviceModel(id:DeviceModel):DeviceModelProfile {
  const profile=deviceModels.find(model=>model.id===id);
  if(!profile)throw new Error(`Unknown device model: ${id}`);
  return profile;
}
/** Match the reference lineup without pretending another simulator is that model. */
export function resolveDeviceModel(platform:string,name:string):DeviceModel|null {
  if(platform!=="ios")return null;
  const normalized=name.trim().replace(/\s+/g," ");
  if(/^iPhone 18 Pro Max$/i.test(normalized))return "iphone-18-pro-max";
  if(/^iPhone 18 Pro$/i.test(normalized))return "iphone-18-pro";
  if(/^iPhone Duo$/i.test(normalized))return "iphone-duo";
  if(/^iPad Pro 13(?:-inch| inch)?\s*\(M5\)$/i.test(normalized))return "ipad-pro-13-m5";
  return null;
}