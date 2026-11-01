import {deviceModel,type DeviceModel,type DeviceFinish,type DeviceDisplay} from "./models";
export type {DeviceModel} from "./models";
export interface DevicePose {model:DeviceModel;finish:DeviceFinish;keyboard:boolean;duoBook:boolean;display:DeviceDisplay;yaw:number;pitch:number;hinge:number;zoom:number;flat:boolean}
export interface DeviceScreenHit {x:number;y:number;display:DeviceDisplay}
export const initialDevicePose=(model:DeviceModel="iphone-18-pro"):DevicePose=>({model,finish:deviceModel(model).defaultFinish,keyboard:false,duoBook:model==="iphone-duo",display:"inner",yaw:0,pitch:0,hinge:model==="iphone-duo"?90:180,zoom:1,flat:false});
export function selectDeviceModel(pose:DevicePose,model:DeviceModel):DevicePose {
  const profile=deviceModel(model);
  return {...initialDevicePose(model),finish:profile.finishes.includes(pose.finish)?pose.finish:profile.defaultFinish,keyboard:profile.keyboard&&pose.keyboard};
}
export function orbitDevice(pose:DevicePose,dx:number,dy:number):DevicePose {
  if(!Number.isFinite(dx)||!Number.isFinite(dy))return pose;
  const pitchLimit=pose.model==="iphone-duo"?Math.PI/2:1.3;
  return {...pose,yaw:((pose.yaw+dx*.006+Math.PI)%(2*Math.PI)+2*Math.PI)%(2*Math.PI)-Math.PI,pitch:Math.max(-pitchLimit,Math.min(pitchLimit,pose.pitch+dy*.006))};
}
export function deviceHinge(value:number):number {return Number.isFinite(value)?Math.max(0,Math.min(180,value)):180;}
export function faceDevice(pose:DevicePose,back=false):DevicePose {
  // The portrait Duo closes around a horizontal hinge. Its cover faces down
  // at full closure; a yaw-only turn would show the thin edge instead.
  if(pose.model==="iphone-duo"&&(pose.display==="cover"||pose.hinge<=20)){
    if(pose.duoBook)return {...pose,yaw:Math.PI/2+pose.hinge*Math.PI/360+(back?Math.PI:0),pitch:0};
    if(pose.hinge<=20)return {...pose,yaw:back?Math.PI:0,pitch:-Math.PI/2};
    return {...pose,yaw:back?0:Math.PI,pitch:(180-pose.hinge)*Math.PI/360};
  }
  return {...pose,yaw:back?Math.PI:0,pitch:0};
}
export function zoomDevice(pose:DevicePose,delta:number):DevicePose {
  if(!Number.isFinite(delta)||pose.flat)return pose;
  return {...pose,zoom:Math.max(.5,Math.min(2,pose.zoom*Math.exp(Math.max(-1000,Math.min(1000,delta))*.0015)))};
}
/** Picking is performed on the actual native model triangles, including body
 * occlusion and contain-fit video. Never reconstruct the geometry in JS. */
export function parseDeviceHit(json:string):DeviceScreenHit|null {
  try {
    const hit=JSON.parse(json);
    if(!hit||![hit.x,hit.y].every(value=>typeof value==="number"&&Number.isFinite(value)&&value>=0&&value<=1)||!['inner','cover'].includes(hit.display))return null;
    return {x:hit.x,y:hit.y,display:hit.display};
  }catch{return null;}
}