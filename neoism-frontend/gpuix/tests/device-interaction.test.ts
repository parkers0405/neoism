import {expect,test} from "bun:test";
import {deviceHinge,faceDevice,initialDevicePose,orbitDevice,zoomDevice,parseDeviceHit,selectDeviceModel} from "../src/device/interaction";
import {deviceModels,deviceModel,deviceFinishes,resolveDeviceModel} from "../src/device/models";
test("the complete T3 device lineup has distinct original model profiles",()=>{
  expect(deviceModels.map(model=>model.id)).toEqual(["iphone-18-pro","iphone-18-pro-max","iphone-duo","ipad-pro-13-m5"]);
  expect(new Set(deviceModels.map(model=>model.label)).size).toBe(4);
  expect(deviceModel("iphone-duo").foldable).toBe(true);
  expect(deviceModel("ipad-pro-13-m5").keyboard).toBe(true);
  for(const model of deviceModels){expect(model.finishes).toContain(model.defaultFinish);for(const id of model.finishes)for(const value of deviceFinishes[id].rgb)expect(value>=0&&value<=1).toBe(true);}
});
test("simulator names resolve to the correct model, not one generic phone",()=>{
  for(const model of deviceModels)expect(resolveDeviceModel("ios",model.label)).toBe(model.id);
  expect(resolveDeviceModel("ios","  IPHONE  18 PRO MAX ")).toBe("iphone-18-pro-max");
  expect(resolveDeviceModel("ios","iPhone 17")).toBeNull();
  expect(resolveDeviceModel("android","iPhone 18 Pro")).toBeNull();
});
test("model selection resets incompatible hardware while preserving compatible finishes",()=>{
  const tablet={...initialDevicePose("ipad-pro-13-m5"),keyboard:true};
  const phone=selectDeviceModel(tablet,"iphone-18-pro-max");
  expect(phone.keyboard).toBe(false);expect(phone.finish).toBe("burgundy");expect(phone.display).toBe("inner");
  expect(selectDeviceModel({...phone,finish:"graphite"},"iphone-duo").finish).toBe("graphite");
});
test("device orbit stays bounded and rejects invalid events",()=>{
  const pose=initialDevicePose();expect(orbitDevice(pose,NaN,1)).toBe(pose);
  const next=orbitDevice(pose,1e6,1e6);expect(next.pitch).toBe(1.3);expect(Math.abs(next.yaw)).toBeLessThanOrEqual(Math.PI);expect(pose.pitch).toBe(0);
});
test("hinge limits match a physical folding display",()=>{expect(deviceHinge(-1)).toBe(0);expect(deviceHinge(190)).toBe(180);expect(deviceHinge(90)).toBe(90);expect(deviceHinge(NaN)).toBe(180);});
test("front/back views follow the actual folded cover normal instead of showing its edge",()=>{
  const closed={...initialDevicePose("iphone-duo"),display:"cover" as const,hinge:0};
  expect(faceDevice(closed).yaw).toBe(Math.PI/2);expect(faceDevice(closed).pitch).toBe(0);
  const portrait={...closed,duoBook:false};expect(faceDevice(portrait).yaw).toBe(0);expect(faceDevice(portrait).pitch).toBe(-Math.PI/2);expect(faceDevice(portrait,true).yaw).toBe(Math.PI);
  expect(orbitDevice(faceDevice(portrait),10,0).pitch).toBe(-Math.PI/2);
  expect(faceDevice({...closed,hinge:180}).yaw).toBe(Math.PI);
  expect(faceDevice(initialDevicePose("iphone-18-pro-max")).yaw).toBe(0);
});
test("viewport zoom is bounded and does not distort flat video",()=>{
  const pose=initialDevicePose();expect(zoomDevice(pose,10000).zoom).toBe(2);expect(zoomDevice(pose,-10000).zoom).toBe(.5);
  expect(zoomDevice(pose,NaN)).toBe(pose);const flat={...pose,flat:true};expect(zoomDevice(flat,100)).toBe(flat);
});
test("native picks preserve display identity and reject invalid geometry results",()=>{
  expect(parseDeviceHit('{"x":0.4,"y":0.6,"display":"cover"}')).toEqual({x:.4,y:.6,display:"cover"});
  for(const value of ['null','bad','{}','{"x":-1,"y":0,"display":"inner"}','{"x":0,"y":2,"display":"inner"}','{"x":0,"y":0,"display":"keys"}'])expect(parseDeviceHit(value)).toBeNull();
});