// Dev-only: lets the UI run in a plain browser for visual review. Never bundled in production builds.
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
    invoke: async (cmd: string)=>{
      if(cmd==="list_servers") return added?[{id:"1",name:"CoA-Repack",path:report.path}]:[];
      if(cmd==="scan_server") return report;
      if(cmd==="add_server"){added=true;return {id:"1",name:"CoA-Repack",path:report.path};}
      if(cmd==="server_status"){const s=running?"running":"stopped";return {observed:{mysql:svc("mysql",3307,s),auth:svc("auth",3724,s),world:svc("world",8085,s)},busy:false,path_exists:true};}
      if(cmd==="start_server"){await new Promise((r: any)=>setTimeout(r,1500));running=true;return {ok:true,exit_code:0,code:null,human:null,output:""};}
      if(cmd==="stop_server"){await new Promise((r: any)=>setTimeout(r,1500));running=false;return {ok:true,exit_code:0,code:null,human:null,output:""};}
      if(cmd.startsWith("plugin:dialog")) return "C:\games\CoA-Repack";
      if(cmd==="plugin:event|listen") return 1;
      if(cmd==="plugin:event|unlisten") return null;
      throw new Error("unmocked "+cmd);
    }
  };
})();
