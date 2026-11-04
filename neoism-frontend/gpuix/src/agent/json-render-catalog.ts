import {defineCatalog,defineSchema} from "@json-render/core";
import {z} from "zod";

// The chat transcript is a Rust canvas, not a DOM tree. This catalog generates
// its model-facing contract; the shared native renderer validates untrusted specs.
const text=z.string().refine(value=>Array.from(value).length<=2000,"At most 2000 Unicode scalars");
const color=z.string().regex(/^#[0-9a-fA-F]{6}$/);
const diagramId=z.string().regex(/^[A-Za-z0-9_-]{1,64}$/);
const diagramLabel=z.string().refine(value=>Array.from(value).length>=1&&Array.from(value).length<=160,"1 to 160 Unicode scalars");
const diagramEdgeLabel=z.string().refine(value=>Array.from(value).length<=80,"At most 80 Unicode scalars");
const components={
  Stack:{props:z.object({}).strict(),description:"Vertical stack of children.",slots:["default"]},
  Card:{props:z.object({title:text.optional(),description:text.optional()}).strict(),description:"Borderless content group with optional heading and description.",slots:["default"]},
  Text:{props:z.object({text,color:color.optional()}).strict(),description:"Plain text, not HTML or Markdown. Optional color changes text color."},
  Metric:{props:z.object({label:text,value:text,detail:text.optional(),change:text.optional(),changeType:z.enum(["positive","negative","neutral"]).optional(),color:color.optional()}).strict(),description:"A prominent value with a label, optional detail, and optional change and changeType (independently optional). Optional color accents the value."},
  Badge:{props:z.object({text,tone:z.enum(["neutral","success","warning","danger"]).optional(),color:color.optional()}).strict(),description:"Status badge; tone defaults to neutral. Optional color overrides tone with matching text/dot and a subtle tinted fill."},
  Progress:{props:z.object({label:text,value:z.number().min(0).max(100),color:color.optional()}).strict(),description:"Labeled progress bar; value is a percentage from 0 to 100. Optional color accents the bar."},
  LineGraph:{props:z.object({title:text.optional(),data:z.array(z.object({label:text,value:z.number().finite()}).strict()).min(2).max(32),color:color.optional()}).strict(),description:"Native line chart with 2 to 32 labeled data points; values must be finite numbers. Hover shows a tooltip; points are selectable locally. Optional color accents the line, points and fading fill."},
  Diagram:{props:z.object({title:text.optional(),direction:z.enum(["LR","TB"]).optional(),nodes:z.array(z.object({id:diagramId,label:diagramLabel,shape:z.enum(["rect","rounded","diamond"]).optional(),color:color.optional()}).strict()).min(1).max(32),edges:z.array(z.object({from:diagramId,to:diagramId,label:diagramEdgeLabel.optional(),color:color.optional()}).strict()).max(64)}).strict().refine(props=>{
    const ids=new Set(props.nodes.map(node=>node.id));
    return ids.size===props.nodes.length&&props.edges.every(edge=>ids.has(edge.from)&&ids.has(edge.to))&&new Set(props.edges.map(edge=>`${edge.from}:${edge.to}`)).size===props.edges.length;
  },"Node IDs and directed edges must be unique; every endpoint must exist"),description:"Native flowchart canvas with automatic layout, zoom in/out, fit-to-view and drag-to-pan. No children. Use 1 to 32 nodes and 0 to 64 directed edges. Node IDs are 1 to 64 ASCII letters, digits, underscores or hyphens; IDs and directed edges must be unique, and endpoints must exist. Node labels are 1 to 160 Unicode scalars; edge labels at most 80. Cycles and self-loops are allowed. Direction defaults to LR; shapes default to rounded. Structured graph data, NOT Mermaid syntax. All canvas interactions are local."},
  Separator:{props:z.object({}).strict(),description:"Horizontal divider with no children."},
  Tabs:{props:z.object({labels:z.array(text).min(2).max(8),defaultTab:z.number().int().min(0).max(7).optional()}).strict(),description:"Local tabs with 2 to 8 string labels; defaultTab is a zero-based integer. Exactly one child per label. Native validation checks children count equals labels length and defaultTab is below labels length (the prop schema only bounds it to 0..7).",slots:["default"]},
  Collapsible:{props:z.object({title:text,defaultOpen:z.boolean().optional()}).strict(),description:"Locally expand or collapse children.",slots:["default"]},
  Filter:{props:z.object({label:text,placeholder:text.optional()}).strict(),description:"Local text search filters direct child subtrees by their text.",slots:["default"]},
  Input:{props:z.object({label:text,placeholder:text.optional(),defaultValue:text.optional(),required:z.boolean().optional(),multiline:z.boolean().optional()}).strict(),description:"Local typed text field; no children."},
  Select:{props:z.object({label:text,options:z.array(text).min(1).max(16).refine(options=>new Set(options).size===options.length,"Options must be unique"),defaultValue:text.optional(),required:z.boolean().optional()}).strict().refine(props=>props.defaultValue===undefined||props.options.includes(props.defaultValue),"defaultValue must be in options"),description:"Local choice field with 1 to 16 unique options; defaultValue must be in options. No children."},
  Checkbox:{props:z.object({label:text,defaultChecked:z.boolean().optional(),required:z.boolean().optional()}).strict(),description:"Local boolean field; no children."},
};

export const jsonRenderExample={root:"summary",elements:{
  summary:{type:"Card",props:{title:"Team performance",description:"Sample data"},children:["revenue","trend","divider","deals","retention"]},
  revenue:{type:"Metric",props:{label:"Weekly revenue",value:"$12,400",change:"+18%",changeType:"positive",color:"#60A5FA"}},
  trend:{type:"LineGraph",props:{title:"Revenue trend",color:"#60A5FA",data:[{label:"Mon",value:1200},{label:"Tue",value:1800},{label:"Wed",value:1400},{label:"Thu",value:2200},{label:"Fri",value:1900},{label:"Sat",value:1700},{label:"Sun",value:2200}]}},
  divider:{type:"Separator",props:{}},
  deals:{type:"Progress",props:{label:"Deals closed",value:72,color:"#34D399"}},
  retention:{type:"Progress",props:{label:"Retention",value:91,color:"#A78BFA"}},
}};

export const jsonRenderInteractiveExample={root:"interactive",elements:{
  interactive:{type:"Card",props:{title:"Explore sample data",description:"Sample data"},children:["tabs"]},
  tabs:{type:"Tabs",props:{labels:["Explore","Options"],defaultTab:0},children:["explore","options"]},
  explore:{type:"Stack",props:{},children:["chart","details"]},
  chart:{type:"LineGraph",props:{title:"Weekly trend",color:"#60A5FA",data:[{label:"Mon",value:12},{label:"Tue",value:18},{label:"Wed",value:14}]}},
  details:{type:"Collapsible",props:{title:"Search team notes",defaultOpen:true},children:["filter"]},
  filter:{type:"Filter",props:{label:"Search notes locally",placeholder:"Team or topic"},children:["sales","support"]},
  sales:{type:"Text",props:{text:"Sales: follow up on renewals"}},
  support:{type:"Text",props:{text:"Support: review response times"}},
  options:{type:"Stack",props:{},children:["topic","scope","includeNotes"]},
  topic:{type:"Input",props:{label:"Topic",placeholder:"Local notes",defaultValue:"Weekly performance",multiline:true}},
  scope:{type:"Select",props:{label:"Team",options:["Sales","Support"],defaultValue:"Sales"}},
  includeNotes:{type:"Checkbox",props:{label:"Include team notes",defaultChecked:false}},
}};

export const jsonRenderDiagramExample={root:"flow",elements:{flow:{type:"Diagram",props:{
  title:"Data pipeline",direction:"LR",
  nodes:[{id:"source",label:"Data",shape:"rounded",color:"#60A5FA"},{id:"check",label:"Valid?",shape:"diamond",color:"#FBBF24"},{id:"transform",label:"Transform",shape:"rect",color:"#A78BFA"},{id:"output",label:"Output",shape:"rounded",color:"#34D399"}],
  edges:[{from:"source",to:"check"},{from:"check",to:"transform",label:"Valid",color:"#34D399"},{from:"transform",to:"output"},{from:"check",to:"source",label:"Retry",color:"#FBBF24"}],
}}}};

const schema=defineSchema(s=>({
  spec:s.object({root:s.string(),elements:s.record(s.object({type:s.ref("catalog.components"),props:s.propsOf("catalog.components"),children:{...s.array(s.string()),...s.optional()}}))}),
  catalog:s.object({components:s.map({props:s.zod(),description:s.string(),slots:s.array(s.string())})}),
}),{promptTemplate:({formatZodType})=>[
  "# Native json-render chat UI",
  "",
   "Neoism graphical chat can render native UI with local interactions from json-render specs. When the user asks for a dashboard, chart, flowchart, architecture diagram, visual status summary, cards, or to try json-render, emit a complete JSON spec in a fenced block with the exact language `json-render`. Always close the fence. Ordinary `json` fences remain source code. Use this when a visual helps, not for every response. Text-only clients may display the source instead.",
  "",
   'The format is {"root":"id","elements":{"id":{"type":"Card","props":{},"children":["child-id"]}}}. Each element has only type, props, and optional children. Only Stack, Card, Tabs, Collapsible, and Filter accept children. Input, Select, Checkbox, and Diagram have no children. Omit absent optional props; do not use null. The root must exist, and all elements must belong to one tree: no missing references, cycles, shared children, or unreachable elements.',
  "",
  "Available components and their exact props:",
  ...Object.entries(components).map(([name,component])=>`- ${name}: ${formatZodType(component.props)}. ${component.description}`),
  "",
   "Limits: 32768 UTF-8 bytes per spec, 64 elements, tree depth 8 including the root, and 2000 Unicode scalars per string. Keep specs small. Props are literal values only: no state bindings, expressions, generic actions, events, URLs, generated JavaScript, scripts, HTML, arbitrary styles, or network requests. No direct shell/tool/network action names. Invalid or incomplete specs display as source.",
   "",
   "Native charts have hover tooltips and selectable points. Tabs, collapse, filter, diagrams, and typed fields are local only. Agent requests belong in the chat composer, NEVER inside generated UI. Do not create Ask agent sections, request forms, submit buttons, prompt properties, or tool actions. Form and Button are not supported. Local field values are not sent anywhere. Do not collect passwords or API keys; use Neoism's built-in credential flow instead.",
   "",
   "Use color purposefully: optional color is a literal six-digit RGB hex string such as #60A5FA, #34D399, #FBBF24, or #A78BFA. It is supported on Text, Metric, Badge, Progress, LineGraph, and individual Diagram nodes and edges. Diagram node colors tint their fill and outline while labels retain readable foreground text; edge colors tint arrows and labels. There is no arbitrary CSS or style object. Keep contrast readable and use a small coherent palette.",
   "",
   "Keep layouts minimal: prefer Stack, use Card only for a distinct titled group, avoid unnecessary nested groups and separators, and never repeat a heading above a Collapsible with the same label. Use LineGraph for trends and Metric change and changeType for deltas. Use Diagram for Mermaid-like flowcharts and architecture graphs: emit nodes/edges, not Mermaid text, HTML or SVG. Keep node labels concise; use TB for tall flows or LR for left-to-right flows. Diagram navigation never requests tools or changes the graph. Use real evidence for results and label example data once in the card description as Sample data; do not repeat sample in every label. Do not invent test outcomes or claim to have visually inspected the UI. Describe local interactions accurately, not a live data connection. For revisions emit a new complete spec, not JSONL patches. A short explanation can accompany the fence.",
  "",
  "Example (illustrative values):",
  "```json-render",
   JSON.stringify(jsonRenderExample,null,2),
   "```",
   "",
   "Interactive example (local controls only):",
   "```json-render",
    JSON.stringify(jsonRenderInteractiveExample),
    "```",
    "",
    "Diagram example (local zoom, fit and pan):",
    "```json-render",
    JSON.stringify(jsonRenderDiagramExample),
    "```",
    "",
].join("\n")});

export const jsonRenderCatalog=defineCatalog(schema,{components});
export const jsonRenderPrompt=()=>jsonRenderCatalog.prompt();