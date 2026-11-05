import {expect,test} from "bun:test";
import {readFileSync} from "node:fs";
import {jsonRenderCatalog,jsonRenderExample,jsonRenderInteractiveExample,jsonRenderDiagramExample,jsonRenderPrompt} from "../src/agent/json-render-catalog";

// Core permits dynamic props; test the original literal-only Zod definitions.
const valid=(type:string,props:unknown)=>{
  const components=jsonRenderCatalog.data.components;
  return Object.hasOwn(components,type)&&components[type as keyof typeof components].props.safeParse(props).success;
};

test("native chat catalog uses json-render core and keeps the on-demand bundled docs in sync",()=>{
  expect(jsonRenderCatalog.componentNames).toEqual(["Stack","Card","Text","Metric","Badge","Progress","LineGraph","Diagram","Separator","Tabs","Collapsible","Filter","Input","Select","Checkbox"]);
  expect(jsonRenderCatalog.actionNames).toEqual([]);
  expect(jsonRenderCatalog.validate(jsonRenderExample).success).toBe(true);
  const prompt=readFileSync(new URL("../../../vendor/neoism-docs/content/Neoism Agent/JSON Render.md",import.meta.url),"utf8");
  expect(prompt.trimEnd()).toBe(jsonRenderPrompt().trimEnd());
  const example=JSON.parse(prompt.split("```json-render\n")[1]!.split("\n```")[0]!);
  expect(example).toEqual(jsonRenderExample);
  expect(JSON.parse(prompt.split("```json-render\n")[2]!.split("\n```")[0]!)).toEqual(jsonRenderInteractiveExample);
  expect(JSON.parse(prompt.split("```json-render\n")[3]!.split("\n```")[0]!)).toEqual(jsonRenderDiagramExample);
});

test("diagram props are bounded literal graphs, not Mermaid scripts or executable actions",()=>{
  const props=jsonRenderDiagramExample.elements.flow.props;
  expect(jsonRenderCatalog.validate(jsonRenderDiagramExample).success).toBe(true);
  expect(valid("Diagram",props)).toBe(true);
  expect(jsonRenderCatalog.data.components.Diagram).not.toHaveProperty("slots");
  const nodes=Array.from({length:32},(_,i)=>({id:`n${i}`,label:"😀".repeat(160)}));
  const edges=Array.from({length:64},(_,i)=>({from:`n${Math.floor(i/32)}`,to:`n${i%32}`,label:"😀".repeat(80)}));
  expect(valid("Diagram",{nodes,edges,direction:"TB"})).toBe(true);
  for(const shape of ["rect","rounded","diamond"])expect(valid("Diagram",{nodes:[{id:"a",label:"A",shape}],edges:[{from:"a",to:"a"}]})).toBe(true);
  for(const bad of [
    {...props,mermaid:"flowchart LR; A-->B"},{...props,onClick:"bash"},{...props,children:[]},
    {...props,title:null},{...props,direction:"RL"},{...props,direction:null},
    {...props,nodes:[]},{...props,nodes:[...nodes,{id:"extra",label:"extra"}]},
    {nodes,edges:[...edges,{from:"n2",to:"n0"}]},
    {nodes:[{id:"a",label:"A"},{id:"a",label:"Duplicate"}],edges:[]},
    {nodes:[{id:"a",label:"A"}],edges:[{from:"a",to:"missing"}]},
    {nodes:[{id:"a",label:"A"}],edges:[{from:"a",to:"a"},{from:"a",to:"a"}]},
    ...["","bad id","é","x".repeat(65)].map(id=>({nodes:[{id,label:"A"}],edges:[]})),
    ...["","😀".repeat(161),null,{$state:"label"}].map(label=>({nodes:[{id:"a",label}],edges:[]})),
    ...[null,"circle"].map(shape=>({nodes:[{id:"a",label:"A",shape}],edges:[]})),
    ...[null,"😀".repeat(81)].map(label=>({nodes:[{id:"a",label:"A"}],edges:[{from:"a",to:"a",label}]})),
    {nodes:[{id:"a",label:"A",url:"https://example.com"}],edges:[]},
    {nodes:[{id:"a",label:"A"}],edges:[{from:"a",to:"a",action:"run"}]},
  ])expect(valid("Diagram",bad)).toBe(false);
});

test("interactive example is a bounded unique tree with literal native props",()=>{
  const spec=jsonRenderInteractiveExample;
  expect(jsonRenderCatalog.validate(spec).success).toBe(true);
  expect(Buffer.byteLength(JSON.stringify(spec),"utf8")).toBeLessThanOrEqual(32768);
  expect(Object.keys(spec.elements).length).toBeLessThanOrEqual(64);
  const seen=new Set<string>();
  const visit=(id:string,depth:number)=>{
    expect(depth).toBeLessThanOrEqual(8);
    expect(seen.has(id)).toBe(false);
    seen.add(id);
    const node=spec.elements[id as keyof typeof spec.elements];
    expect(node).toBeDefined();
    expect(valid(node.type,node.props)).toBe(true);
    const children="children" in node?node.children:[];
    if(node.type==="Tabs"){
      const props=jsonRenderCatalog.data.components.Tabs.props.parse(node.props);
      expect(children.length).toBe(props.labels.length);
      expect(props.defaultTab??0).toBeLessThan(props.labels.length);
    }
    if(["Input","Select","Checkbox"].includes(node.type))expect(children).toEqual([]);
    for(const child of children)visit(child,depth+1);
  };
  visit(spec.root,1);
  expect(seen.size).toBe(Object.keys(spec.elements).length);
  expect(JSON.stringify(spec).match(/Sample data/g)).toHaveLength(1);
});

test("interactive literal prop schemas enforce bounds, types, and local fields",()=>{
  for(const [type,props] of [
    ["Tabs",{labels:["One","Two"]}],
    ["Tabs",{labels:Array.from({length:8},(_,i)=>String(i)),defaultTab:7}],
    ["Collapsible",{title:"Details",defaultOpen:false}],
    ["Filter",{label:"Search",placeholder:"Text"}],
    ["Input",{label:"Name",placeholder:"Name",defaultValue:"😀".repeat(2000),required:true,multiline:false}],
    ["Select",{label:"Team",options:["A"],defaultValue:"A",required:true}],
    ["Select",{label:"Team",options:Array.from({length:16},(_,i)=>String(i))}],
    ["Checkbox",{label:"Confirm",defaultChecked:false,required:true}],
  ])expect(valid(type as string,props)).toBe(true);
  for(const [type,props] of [
    ["Tabs",{labels:["One"]}],
    ["Tabs",{labels:Array(9).fill("Tab")}],
    ...[-1,8,0.5,"0",null].map(defaultTab=>["Tabs",{labels:["A","B"],defaultTab}]),
    ["Tabs",{labels:["A",2]}],
    ["Collapsible",{title:"Details",defaultOpen:"true"}],
    ["Filter",{label:"Search",placeholder:null}],
    ["Form",{submitLabel:"Review"}],
    ["Form",{prompt:"Analyze"}],
    ["Form",{submitLabel:"Review",prompt:{$state:"prompt"}}],
    ["Form",{submitLabel:"Review",prompt:"Analyze",fields:[]}],
    ["Input",{label:"Name",defaultValue:1}],
    ["Input",{label:"Name",required:"true"}],
    ["Input",{label:"Name",multiline:1}],
    ["Select",{label:"Team",options:[]}],
    ["Select",{label:"Team",options:Array.from({length:17},(_,i)=>String(i))}],
    ["Select",{label:"Team",options:["A","A"]}],
    ["Select",{label:"Team",options:["A"],defaultValue:"B"}],
    ["Select",{label:"Team",options:[1]}],
    ["Select",{label:"Team",options:["A"],required:1}],
    ["Checkbox",{label:"Confirm",defaultChecked:"false"}],
    ["Checkbox",{label:"Confirm",required:null}],
    ["Button",{label:"Review",prompt:"Analyze",variant:"danger"}],
    ["Button",{label:"Review",prompt:"Analyze",disabled:1}],
    ["Button",{label:"Review",prompt:"Analyze",action:"bash"}],
    ["Button",{label:"Review",prompt:"Analyze",onClick:"fetch"}],
  ])expect(valid(type as string,props)).toBe(false);
  for(const type of ["Tabs","Collapsible","Filter","Input","Select","Checkbox"]){
    const node=Object.values(jsonRenderInteractiveExample.elements).find(node=>node.type===type)!;
    for(const [key,value] of Object.entries(node.props)){
      if(typeof value==="string"){
        expect(valid(type,{...node.props,[key]:"😀".repeat(2001)})).toBe(false);
        expect(valid(type,{...node.props,[key]:null})).toBe(false);
        expect(valid(type,{...node.props,[key]:{$state:"dynamic"}})).toBe(false);
      }
    }
    expect(valid(type,{...node.props,children:[]})).toBe(false);
    expect(valid(type,{...node.props,action:"shell"})).toBe(false);
  }
  expect(valid("Tabs",{labels:["😀".repeat(2000),"B"]})).toBe(true);
  expect(valid("Tabs",{labels:["😀".repeat(2001),"B"]})).toBe(false);
  expect(valid("Select",{label:"Choice",options:["😀".repeat(2000)]})).toBe(true);
  expect(valid("Select",{label:"Choice",options:["😀".repeat(2001)]})).toBe(false);
  for(const type of ["Input","Select","Checkbox"]){
    expect(jsonRenderCatalog.data.components[type as "Input"]).not.toHaveProperty("slots");
  }
});

test("literal colors are supported but agent request components are not",()=>{
  for(const [type,props] of [
    ["Text",{text:"Blue text"}],["Metric",{label:"Count",value:"12"}],
    ["Badge",{text:"Partly cloudy"}],["Progress",{label:"Done",value:42}],
    ["LineGraph",{data:[{label:"A",value:1},{label:"B",value:2}]}],
  ] as const){
    for(const color of ["#60A5FA","#34d399","#000000","#FFFFFF"])expect(valid(type,{...props,color})).toBe(true);
    for(const color of [null,"red","#fff","#12345678","#zzzzzz",{$state:"color"}])expect(valid(type,{...props,color})).toBe(false);
  }
  expect(valid("Form",{submitLabel:"Send",prompt:"Do something"})).toBe(false);
  expect(valid("Button",{label:"Ask agent",prompt:"Do something"})).toBe(false);
  const prompt=jsonRenderPrompt();
  expect(prompt).toContain("Agent requests belong in the chat composer, NEVER inside generated UI");
  expect(prompt).not.toContain("- Form:");expect(prompt).not.toContain("- Button:");
  expect(JSON.stringify(jsonRenderInteractiveExample)).not.toContain("Ask agent");
});

test("native catalog prop schemas reject invented components, invalid props, actions, and dynamic bindings",()=>{
  // Native validation also enforces the tree constraints outside these schemas.
  expect(valid("Stack",{})).toBe(true);
  expect(valid("Text",{text:"A native label"})).toBe(true);
  expect(valid("Progress",{label:"Done",value:100})).toBe(true);
  expect(valid("Separator",{})).toBe(true);
  for(const extra of [{},{change:"+18%"},{changeType:"positive"},{change:"-2%",changeType:"negative"},{change:"0%",changeType:"neutral"}]){
    expect(valid("Metric",{label:"Revenue",value:"$12,400",...extra})).toBe(true);
  }
  const points=Array.from({length:32},(_,i)=>({label:String(i),value:i-16}));
  expect(valid("LineGraph",{data:points.slice(0,2)})).toBe(true);
  expect(valid("LineGraph",{title:"Trend",data:points})).toBe(true);
  expect(valid("LineGraph",{title:"😀".repeat(2000),data:[{label:"😀".repeat(2000),value:-1.5},{label:"End",value:0}]})).toBe(true);
  expect(valid("Metric",{label:"Revenue",value:"$12,400",change:"😀".repeat(2000)})).toBe(true);
  expect(jsonRenderCatalog.data.components.LineGraph).not.toHaveProperty("slots");
  expect(jsonRenderCatalog.data.components.Separator).not.toHaveProperty("slots");
  for(const [type,props] of [
    ["Button",{label:"Run",action:"bash"}],
    ["Text",{text:{$state:"secret"}}],
    ["Text",{text:"label",onClick:"run"}],
    ["Text",{text:"x".repeat(2001)}],
    ["Card",{title:null}],
    ["Progress",{label:"Done",value:101}],
    ["Progress",{label:"Done",value:-1}],
    ["Badge",{text:"Status",tone:"invented"}],
    ["Metric",{label:"Revenue",value:"1",change:18}],
    ["Metric",{label:"Revenue",value:"1",change:null}],
    ["Metric",{label:"Revenue",value:"1",change:"😀".repeat(2001)}],
    ["Metric",{label:"Revenue",value:"1",changeType:"success"}],
    ["Metric",{label:"Revenue",value:"1",changeType:null}],
    ["Separator",{title:"Divider"}],
    ["Separator",{children:[]}],
    ["LineGraph",{data:[]}],
    ["LineGraph",{data:points.slice(0,1)}],
    ["LineGraph",{data:[...points,points[0]]}],
    ["LineGraph",{data:points,title:"😀".repeat(2001)}],
    ["LineGraph",{data:[{label:"😀".repeat(2001),value:1},points[0]]}],
    ["LineGraph",{data:[{label:"Start",value:Infinity},points[0]]}],
    ["LineGraph",{data:[{label:"Start",value:-Infinity},points[0]]}],
    ["LineGraph",{data:[{label:"Start",value:NaN},points[0]]}],
    ["LineGraph",{data:[{label:"Start",value:"1"},points[0]]}],
    ["LineGraph",{data:[{label:"Start",value:1,color:"red"},points[0]]}],
    ["LineGraph",{data:points,color:"red"}],
    ["LineGraph",{data:points,children:[]}],
    ["LineGraph",{data:{$state:"points"}}],
  ] as const)expect(valid(type,props)).toBe(false);
});