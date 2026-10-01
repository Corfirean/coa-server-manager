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
      if(cmd==="check_update") return {from_version:(window as any).__updated?"0.261001.8":"0.260930.5",to_version:"0.261001.8",items:[],conflicts:[],migrations:5,download_bytes:6000000};
      if(cmd==="apply_update"){const w=window as any;for(let i=1;i<=8;i++){await new Promise((r: any)=>setTimeout(r,700));(w.__handlers?.["update-progress"]??[]).forEach((h: any)=>h({event:"update-progress",id:1,payload:{step:i<4?"Downloading the update":i<7?"Applying the update":"Finishing",percent:i*12}}));}(window as any).__updated=true;return {txn:{state:"committed",to_version:"0.261001.8"},migrations:null};}
      if(cmd==="get_population"){if(!running) return null;let b=(window as any).__bots??85;const t=(window as any).__target??0;if(t>b){b=Math.min(t,b+25);(window as any).__bots=b;}return {online_total:b,bots_online:Math.max(0,b-7),players_online:0,bots_total:b};}
      if(cmd==="get_performance") return running?{mean_ms:14,median_ms:15,p95_ms:24,p99_ms:30,max_ms:61,ticks_per_sec:71.4}:null;
      if(cmd==="run_diagnostics") return {problems:2,checks:[{id:"files",title:"Server files",level:"ok",detail:"All expected server parts were found."},{id:"world",title:"World server",level:"warn",detail:"Not running."},{id:"disk",title:"Free disk space",level:"warn",detail:"Only 3 GB free; backups and updates need room."},{id:"exposure",title:"Private services",level:"ok",detail:"Database and server console are not reachable from the network."}]};
      if(cmd.startsWith("client_")){
        const w=window as any;
        const emit=(payload: any)=>(w.__handlers?.["client-progress"]??[]).forEach((h: any)=>h({event:"client-progress",id:1,payload}));
        const sleep=(ms: number)=>new Promise((r: any)=>setTimeout(r,ms));
        const total=46404000000;
        const run=async(phase: string,n: number,speed: number)=>{for(let i=1;i<=n;i++){if(w.__stop){w.__stop=false;throw {human:{code:"unknown",title:"",message:"",actions:[]},technical:"Download cancelled."};}emit({phase,done:Math.round(total*i/n),total,bytes_per_sec:speed,file:phase==="scan"?"Data/patch-"+i+".MPQ":"Data/patch-O.MPQ"});await sleep(120);}};
        if(cmd==="client_status"){const c=w.__client??"none";return {linked:c!=="none",managed:c==="old"||c==="new",installed_version:c==="old"?"2026-09-20-01":c==="new"?"2026-09-29-01":null,latest_version:"2026-09-29-01",latest_bytes:total,update_available:c==="old"};}
        if(cmd==="client_download_check") return {needed_bytes:total,free_bytes:args.parent.startsWith("C")?210e9:20e9,version:"2026-09-29-01",dest:args.parent+"\\CoA Client"};
        if(cmd==="client_plan"){await run("scan",12,0);return {version:"2026-09-29-01",items:[{path:"Data/patch-O.MPQ",size:2814983707,kind:"changed"},{path:"Data/patch-WB1.MPQ",size:2001890268,kind:"missing"},...(w.__modified?[{path:"d3d9.dll",size:4284430,kind:"modified"},{path:"Ascension.exe",size:7694848,kind:"modified"}]:[])],download_bytes:4829000000,total_files:200,up_to_date_files:196,kept_files:0};}
        if(cmd==="client_sync"){await run("download",25,41e6);w.__client="new";return null;}
        if(cmd==="client_download"){await run("scan",4,0);await run("download",40,41e6);w.__client="new";return {path:args.parent+"\\CoA Client",executable:"Ascension.exe",realmlists:[{path:"x",host:"127.0.0.1"}],addon:{installed:true,version:"1",up_to_date:true},other_addons:0};}
        if(cmd==="client_cancel"){w.__stop=true;return null;}
      }
      if(cmd==="realmlist_profiles"){const w=window as any;w.__rl=w.__rl??{profiles:[{id:"solo",name:"Solo",data:"set realmlist 127.0.0.1\r\n"},{id:"ptr",name:"PTR",data:"set realmlist ptr.example.org\r\n"}],active:"solo"};return w.__rl;}
      if(cmd==="realmlist_activate"){(window as any).__rl.active=args.profileId;return [];}
      if(cmd==="realmlist_save"){const w=window as any;const rl=w.__rl;if(args.profileId){const p=rl.profiles.find((x: any)=>x.id===args.profileId);p.name=args.name;p.data=args.data;return p;}if(rl.profiles.some((x: any)=>x.name.toLowerCase()===String(args.name).toLowerCase()))throw {human:{code:"unknown",title:"",message:"",actions:[]},technical:"A realmlist with that name already exists."};const p={id:String(args.name).toLowerCase().replace(/[^a-z0-9]+/g,"-"),name:args.name,data:(/\s/.test(args.data.trim())?args.data.trim():"set realmlist "+args.data.trim())+"\r\n"};rl.profiles.push(p);return p;}
      if(cmd==="realmlist_delete"){const rl=(window as any).__rl;rl.profiles=rl.profiles.filter((x: any)=>x.id!==args.profileId);return null;}
      if(cmd==="modules_list") return [];
      if(cmd==="module_set_enabled"){const m=(window as any).__mods.find((x: any)=>x.id===args.module);m.enabled=args.enabled;return null;}
      if(cmd==="module_settings") return [{key:"WarGames.Enable",value:"1",default:"1",doc:"Turns War Games on."},{key:"WarGames.ChallengeSeconds",value:"60",default:"60",doc:"How long a challenge stays open."}];
      if(cmd==="module_save_settings") return Object.keys(args.changes);
      if(cmd==="report_context") return {manager_version:"0.3.1",windows:"Windows 11 (build 26200)",install_kind:"new",server_version:"0.261001.10"};
      if(cmd==="open_link"){(window as any).__lastLink=args.url;return null;}
      if(cmd==="export_diagnostics") return "C:\\Users\\you\\Desktop\\CoA-Diagnostics-20261001-140000.zip";
      if(cmd==="install_preflight") return {ok:true,problems:[],free_bytes:210*2**30};
      if(cmd==="install_requirements") return {download_bytes:Math.round(5.9*2**30),unpacked_bytes:Math.round(11.4*2**30),version:"0.2.0"};
      if(cmd==="list_accounts"){const w=window as any;w.__acc=w.__acc??[{id:2,name:"ALICE",access:3,online:true,last_login:"2026-10-01 12:04:11",characters:4},{id:5,name:"BOB",access:0,online:false,last_login:null,characters:0},{id:6,name:"CAROL",access:2,online:false,last_login:"2026-09-30 21:40:02",characters:2}];return w.__acc;}
      if(cmd==="account_set_access"){const a=(window as any).__acc.find((x: any)=>x.name===args.name);if(a)a.access=args.level;return null;}
      if(cmd==="account_set_password") return null;
      if(cmd==="account_rename"){const a=(window as any).__acc.find((x: any)=>x.name===args.name);if(a)a.name=String(args.newName).toUpperCase();return null;}
      if(cmd==="client_info"){const c=(window as any).__client??"none";return c==="none"?null:{path:"C:\\games\\CoA Client",executable:"Ascension.exe",realmlists:[{path:"x",host:"127.0.0.1"}],addon:{installed:true,version:"1",up_to_date:true},other_addons:2};}
      if(cmd==="set_client"){(window as any).__client="foreign";return {path:args.path,executable:"Ascension.exe",realmlists:[],addon:{installed:false,version:null,up_to_date:null},other_addons:0};}
      if(cmd==="plugin:event|listen"){const w=window as any;(w.__handlers??={})[args.event]=[...(w.__handlers[args.event]??[]),args.handler];return 1;}
      if(cmd==="plugin:event|unlisten") return null;
      throw new Error("unmocked "+cmd);
    }
  };
})();
