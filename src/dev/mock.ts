// Dev-only: lets the UI run in a plain browser for visual review. Never bundled in production builds.
import botsSchema from "../../schemas/bots.json";
import serverSchema from "../../schemas/server.json";
import botsPresets from "../../schemas/presets/bots.json";
import serverPresets from "../../schemas/presets/server.json";
export {};

(() => {
  let added = false, running = false;
  const report = {path:"C:\games\CoA-Repack",classification:"healthy",items:[
    {key:"worldserver",label:"World server",status:"found",detail:null},
    {key:"authserver",label:"Auth server",status:"found",detail:null},
    {key:"database_content",label:"Auth / Characters / World databases",status:"found",detail:"acore_auth, acore_characters, acore_world"},
    {key:"companions",label:"CoA Companions (bots)",status:"found",detail:"71 settings"},
    {key:"client",label:"Game client",status:"missing",detail:null}],
    worldserver:null,authserver:null,banner_revision:"3567e2f8e9d5",bot_config_keys:71,client:null,
    notes:["worldserver.exe differs from the one listed in RELEASE.json (release 2026-09-11); this looks like a customised build."],modifies_files:false};
  const svc=(n: string,p: number,st: string)=>({name:n,state:st,pid:st==="running"?100:null,port:p,port_ready:st==="running",conflict:null,uptime_secs:st==="running"?4206:null});
  (window as any).__TAURI_INTERNALS__ = {
    transformCallback: (cb: unknown)=>cb, unregisterCallback(){}, convertFileSrc:(x: unknown)=>x,
    invoke: async (cmd: string, args?: any)=>{
      if(cmd==="list_servers") return added?[{id:"1",name:"CoA-Repack",path:report.path}]:[];
      if(cmd==="scan_server") return report;
      if(cmd==="add_server"){added=true;return {id:"1",name:"CoA-Repack",path:report.path};}
      if(cmd==="server_status"){const s=running?"running":"stopped";return {observed:{mysql:svc("mysql",3307,s),auth:svc("auth",3724,s),world:svc("world",8085,s)},busy:false,path_exists:true};}
      if(cmd==="start_server"){await new Promise((r: any)=>setTimeout(r,1500));running=true;return {ok:true,exit_code:0,code:null,human:null,output:""};}
      if(cmd==="stop_server"){await new Promise((r: any)=>setTimeout(r,1500));running=false;return {ok:true,exit_code:0,code:null,human:null,output:""};}
      if(cmd.startsWith("plugin:dialog")) return "C:\games\CoA-Repack";
      if(cmd==="get_settings"){const sc=args?.scope==="server"?serverSchema:botsSchema;return {scope:args?.scope,categories:sc.categories,settings:(sc.settings as any[]).map(x=>({...x,options:x.options??[],value:x.default,is_default:true,present:true,problem:null,drift:false})),unknown_keys:3,drift_keys:[],files:[]};}
      if(cmd==="list_presets"){const pr=args?.scope==="server"?serverPresets:botsPresets;return (pr.presets as any[]).map(p=>({id:p.id,title:p.title,description:p.description}));}
      if(cmd==="preview_preset"){const a=args;const sc=a.scope==="server"?serverSchema:botsSchema;const pr=(a.scope==="server"?serverPresets:botsPresets).presets as any[];const p=pr.find(x=>x.id===a.preset)??{id:a.preset??"defaults",title:"Recommended defaults",description:"Every setting returns to its default value."};const st=(sc.settings as any[]).filter(x=>x.type==="int"||x.type==="float"||x.type==="enum").slice(0,3);return {id:p.id,title:p.title,description:p.description,changes:st.map(x=>({key:x.key,title:x.title,from:x.default,to:x.type==="enum"?(x.options?.[1]?.value??x.default):(Number(x.default)||1)*2,dangerous:false}))};}
      if(cmd==="companion_sizes") return {hardware:{cores:16,ram_gb:31,free_ram_gb:12},sizes:[{id:"small",title:"Small",bots:50,warning:null},{id:"medium",title:"Medium",bots:250,warning:null},{id:"large",title:"Large",bots:500,warning:"500 companions may need more CPU and memory than this computer has (8 cores, 16 GB)."}]};
      if(cmd==="add_companions"){const base=(window as any).__bots??85;(window as any).__target=base+args.count;return {spawned:"queued",created:null,baseline:base};}
      if(cmd==="get_population"){if(!running) return null;let b=(window as any).__bots??85;const t=(window as any).__target??0;if(t>b){b=Math.min(t,b+25);(window as any).__bots=b;}return {online_total:b,bots_online:Math.max(0,b-7),players_online:0,bots_total:b};}
      if(cmd==="get_performance") return running?{mean_ms:14,median_ms:15,p95_ms:24,p99_ms:30,max_ms:61,ticks_per_sec:71.4}:null;
      if(cmd==="run_diagnostics") return {problems:2,checks:[{id:"files",title:"Server files",level:"ok",detail:"All expected server parts were found."},{id:"world",title:"World server",level:"warn",detail:"Not running."},{id:"disk",title:"Free disk space",level:"warn",detail:"Only 3 GB free; backups and updates need room."},{id:"exposure",title:"Private services",level:"ok",detail:"Database and server console are not reachable from the network."}]};
      if(cmd==="plugin:event|listen") return 1;
      if(cmd==="plugin:event|unlisten") return null;
      throw new Error("unmocked "+cmd);
    }
  };
})();
