import java.lang.instrument.Instrumentation;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.atomic.AtomicBoolean;

/** Read-only UI observer. No transformers, setters, focus changes, input calls,
 * authentication reads, or callbacks other than scheduling this observation. */
public final class NativeMinecraftObserver {
    static Path report;
    static long seq;
    static Object call(Object o, String name) throws Exception {
        Method m=o.getClass().getMethod(name); m.trySetAccessible(); return m.invoke(o);
    }
    static String quote(Object value) {
        if(value==null)return "null";
        StringBuilder s=new StringBuilder("\"");
        for(char c:value.toString().toCharArray()) {
            if(c=='"'||c=='\\')s.append('\\').append(c);
            else if(c<32)s.append(String.format("\\u%04x",(int)c)); else s.append(c);
        }
        return s.append('"').toString();
    }
    static String json(Object o) {
        if(o==null)return "null";
        if(o instanceof Number || o instanceof Boolean)return o.toString();
        if(o instanceof Map<?,?> m) {var a=new ArrayList<String>();m.forEach((k,v)->a.add(quote(k)+":"+json(v)));return "{"+String.join(",",a)+"}";}
        if(o instanceof Collection<?> c)return "["+String.join(",",c.stream().map(NativeMinecraftObserver::json).toList())+"]";
        return quote(o);
    }
    static void publish(Map<String,Object> data) throws Exception {
        data.put("seq",++seq);data.put("pid",ProcessHandle.current().pid());
        String text=json(data);
        Path tmp=report.resolveSibling("minecraft-report.tmp");
        Files.writeString(tmp,text);Files.move(tmp,report,StandardCopyOption.REPLACE_EXISTING,StandardCopyOption.ATOMIC_MOVE);
        Files.writeString(report.resolveSibling("minecraft-reports.jsonl"),text+"\n",StandardOpenOption.CREATE,StandardOpenOption.APPEND);
    }
    static void widgets(Object node,List<Object> result,Set<Object> seen,int depth) throws Exception {
        if(node==null || depth>12 || !seen.add(node))return;
        ClassLoader cl=node.getClass().getClassLoader();
        Class<?> widget=Class.forName("net.minecraft.client.gui.components.AbstractWidget",false,cl);
        if(widget.isInstance(node)) {
            var w=new LinkedHashMap<String,Object>();w.put("class",node.getClass().getName());
            for(String n:List.of("getX","getY","getWidth","getHeight","isFocused","isActive"))w.put(n,call(node,n));
            w.put("label",call(call(node,"getMessage"),"getString"));
            w.put("visible",widget.getField("visible").get(node));
            if(Class.forName("net.minecraft.client.gui.components.EditBox",false,cl).isInstance(node))w.put("value",call(node,"getValue"));
            result.add(w);
        }
        try {Object children=call(node,"children");if(children instanceof Iterable<?> list)for(Object c:list)widgets(c,result,seen,depth+1);}
        catch(NoSuchMethodException ignored) {}
    }
    static void observe(Object mc) {
        try {
            Object gui=mc.getClass().getField("gui").get(mc),screen=call(gui,"screen"),win=call(mc,"getWindow");
            var data=new LinkedHashMap<String,Object>();
            data.put("screen",screen==null?null:screen.getClass().getName());
            data.put("overlay",call(gui,"overlay")!=null);
            data.put("focused",call(mc,"isWindowActive"));
            data.put("hasWorld",mc.getClass().getField("level").get(mc)!=null);
            for(String n:List.of("getWidth","getHeight","getGuiScaledWidth","getGuiScaledHeight","getGuiScale"))data.put(n,call(win,n));
            var list=new ArrayList<Object>();widgets(screen,list,Collections.newSetFromMap(new IdentityHashMap<>()),0);data.put("widgets",list);
            publish(data);
        } catch(Throwable e) {error(e);}
    }
    static void error(Throwable e) {
        try {Files.writeString(report.resolveSibling("observer-error.log"),e.toString()+"\n",StandardOpenOption.CREATE,StandardOpenOption.APPEND);}catch(Exception ignored){}
    }
    public static void premain(String args,Instrumentation instrumentation) {
        report=Path.of(args);var pending=new AtomicBoolean();
        Thread thread=new Thread(()->{
            Class<?> clazz=null;
            for(int i=0;i<1600;i++)try {
                Thread.sleep(150);
                if(clazz==null)for(Class<?> c:instrumentation.getAllLoadedClasses())if(c.getName().equals("net.minecraft.client.Minecraft")){clazz=c;break;}
                if(clazz==null)continue;
                Object mc=clazz.getMethod("getInstance").invoke(null);
                if(mc==null || !pending.compareAndSet(false,true))continue;
                clazz.getMethod("execute",Runnable.class).invoke(mc,(Runnable)()->{try{observe(mc);}finally{pending.set(false);}});
            }catch(Throwable e){pending.set(false);error(e);}
        },"neoism-read-only-ui-observer");thread.setDaemon(true);thread.start();
    }
}
