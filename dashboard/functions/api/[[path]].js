const corsHeaders = { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" };
const defaults = { prefix: "-mb", greeting: "hey! i'm Mallow Bot ☁️", greetings: true, mention_only: true };
const cookieName = "mallow_session";
const enc = new TextEncoder();
function json(data, status=200, headers={}) { return new Response(JSON.stringify(data), { status, headers: { ...corsHeaders, ...headers } }); }
function cookie(request, name) { const part=(request.headers.get("Cookie")||"").split(";").map(x=>x.trim()).find(x=>x.startsWith(name+"=")); return part ? decodeURIComponent(part.slice(name.length+1)) : ""; }
function b64(bytes) { let s=""; for(const b of bytes)s+=String.fromCharCode(b); return btoa(s).replace(/=/g,"").replace(/\+/g,"-").replace(/\//g,"_"); }
function unb64(s) { s=s.replace(/-/g,"+").replace(/_/g,"/"); s+="=".repeat((4-s.length%4)%4); return Uint8Array.from(atob(s),c=>c.charCodeAt(0)); }
async function sign(value, secret) { const key=await crypto.subtle.importKey("raw",enc.encode(secret),{name:"HMAC",hash:"SHA-256"},false,["sign"]); return b64(new Uint8Array(await crypto.subtle.sign("HMAC",key,enc.encode(value)))); }
async function token(payload, secret) { const body=b64(enc.encode(JSON.stringify(payload))); return body+"."+await sign(body,secret); }
async function untoken(value, secret) { try { const [body,sig]=value.split("."); if(!body||!sig||!(await sign(body,secret)===sig))return null; const data=JSON.parse(new TextDecoder().decode(unb64(body))); if(data.exp && data.exp<Date.now())return null; return data; } catch { return null; } }
function cookieHeader(value, maxAge) { return cookieName+"="+encodeURIComponent(value)+"; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age="+maxAge; }
async function session(request, env) { const s=await untoken(cookie(request,cookieName),env.SESSION_SECRET||""); if(!s?.user?.id)return null; return s; }
async function settings(env) { await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK (id=1), prefix TEXT NOT NULL, greeting TEXT NOT NULL, greetings INTEGER NOT NULL, mention_only INTEGER NOT NULL, updated_at TEXT NOT NULL)").run(); const row=await env.MALLOW_DB.prepare("SELECT prefix,greeting,greetings,mention_only FROM settings WHERE id=1").first(); return row?{prefix:row.prefix,greeting:row.greeting,greetings:!!row.greetings,mention_only:!!row.mention_only}:defaults; }

 async function authorizedGuild(s, env, guildId) {
  if(!/^\d{17,20}$/.test(guildId||"")) return {error:"Select a valid Discord server.",status:400};
  if(!s?.user?.access_token) return {error:"Sign in again to manage server settings.",status:401};
  const r=await fetch("https://discord.com/api/users/@me/guilds",{headers:{authorization:"Bearer "+s.user.access_token}});
  if(!r.ok) return {error:"Discord sign-in expired. Sign out and sign in again.",status:401};
  const guilds=await r.json();
  const guild=guilds.find(g=>g.id===guildId);
  if(!guild) return {error:"That server is not available to your Discord account.",status:403};
  let permissions=0n;try{permissions=BigInt(guild.permissions||"0")}catch{}
  if((permissions&8n)===0n&&(permissions&32n)===0n) return {error:"You need Administrator or Manage Server permission to configure Mallow here.",status:403};
  if(env.BOT_TOKEN){
   const br=await fetch("https://discord.com/api/guilds/"+guildId,{headers:{authorization:"Bot "+env.BOT_TOKEN}});
   if(!br.ok) return {error:"Mallow Bot must be in this server before you can configure it.",status:409};
  }
  return {guild};
 }
 async function ensureGuildSettings(env){
  await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS guild_settings (guild_id TEXT PRIMARY KEY, prefix TEXT NOT NULL DEFAULT '-mb', greeting TEXT NOT NULL DEFAULT 'hey! i\\'m Mallow Bot ☁️', greetings INTEGER NOT NULL DEFAULT 1, mention_only INTEGER NOT NULL DEFAULT 1, disabled_commands TEXT NOT NULL DEFAULT '[]', updated_at TEXT NOT NULL)").run();
 }

async function route(request, env) {
 const missing=["CLIENT_ID","CLIENT_SECRET","REDIRECT_URI","SESSION_SECRET","BOT_OWNER_ID","MALLOW_DB"].filter(k=>!env[k]); if(missing.length) return json({error:"Dashboard server configuration is incomplete.",missing},503);
 const url=new URL(request.url); const path=url.pathname.replace(/^\/api\/?/,"");
 if(path==="login"&&request.method==="GET"){const state=b64(crypto.getRandomValues(new Uint8Array(24)));const signed=await token({state,exp:Date.now()+10*60*1000},env.SESSION_SECRET);const auth=new URL("https://discord.com/oauth2/authorize");auth.searchParams.set("client_id",env.CLIENT_ID);auth.searchParams.set("redirect_uri",env.REDIRECT_URI);auth.searchParams.set("response_type","code");auth.searchParams.set("scope","identify guilds");auth.searchParams.set("state",state);return Response.redirect(auth.toString(),302);}
 if(path==="callback"&&request.method==="GET"){const err=url.searchParams.get("error");const code=url.searchParams.get("code");const state=url.searchParams.get("state");if(err||!code||!state)return Response.redirect("/?auth=failed",302);
 // State is also stored in a short-lived cookie to prevent login CSRF.
 const saved=await untoken(cookie(request,"mallow_oauth_state"),env.SESSION_SECRET);if(!saved||saved.state!==state)return Response.redirect("/?auth=state",302);
 const form=new URLSearchParams({client_id:env.CLIENT_ID,client_secret:env.CLIENT_SECRET,grant_type:"authorization_code",code,redirect_uri:env.REDIRECT_URI});
 const tr=await fetch("https://discord.com/api/oauth2/token",{method:"POST",headers:{"content-type":"application/x-www-form-urlencoded"},body:form});if(!tr.ok)return Response.redirect("/?auth=token",302);const td=await tr.json();
 const ur=await fetch("https://discord.com/api/users/@me",{headers:{authorization:"Bearer "+td.access_token}});if(!ur.ok)return Response.redirect("/?auth=user",302);const u=await ur.json();const payload={user:{id:u.id,username:u.username,global_name:u.global_name||u.username,access_token:td.access_token,refresh_token:td.refresh_token,token_expires_at:Date.now()+Number(td.expires_in||604800)*1000,avatar_url:u.avatar?"https://cdn.discordapp.com/avatars/"+u.id+"/"+u.avatar+".png":"https://cdn.discordapp.com/embed/avatars/0.png"},exp:Date.now()+7*24*60*60*1000};
 const signed=await token(payload,env.SESSION_SECRET);const h=new Headers({"location":"/","cache-control":"no-store"});h.append("set-cookie",cookieHeader(signed,7*24*60*60));h.append("set-cookie","mallow_oauth_state=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");return new Response(null,{status:302,headers:h});
 }

 if(path==="guild-settings"&&(request.method==="GET"||request.method==="PUT")){
  const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
  const guildId=url.searchParams.get("guild_id")||"";
  const access=await authorizedGuild(s,env,guildId);if(access.error)return json({error:access.error},access.status);
  await ensureGuildSettings(env);
  if(request.method==="GET"){
   let row=await env.MALLOW_DB.prepare("SELECT prefix,greeting,greetings,mention_only,disabled_commands,updated_at FROM guild_settings WHERE guild_id=?").bind(guildId).first();
   if(!row)row={prefix:defaults.prefix,greeting:defaults.greeting,greetings:1,mention_only:1,disabled_commands:"[]",updated_at:null};
   let disabled=[];try{disabled=JSON.parse(row.disabled_commands||"[]");if(!Array.isArray(disabled))disabled=[]}catch{}
   return json({guild:{id:guildId,name:access.guild.name},settings:{prefix:row.prefix,greeting:row.greeting,greetings:!!row.greetings,mention_only:!!row.mention_only,disabled_commands:disabled,updated_at:row.updated_at}});
  }
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const prefix=String(body.prefix??"-mb").trim(),greeting=String(body.greeting??defaults.greeting);
  const disabled=Array.isArray(body.disabled_commands)?[...new Set(body.disabled_commands.map(x=>String(x).trim()).filter(x=>/^[a-z0-9_-]{1,32}$/i.test(x)))].slice(0,200):[];
  if(!prefix||prefix.length>8||/[\\r\\n\\s]/.test(prefix))return json({error:"Prefix must be 1–8 characters with no spaces or line breaks."},400);
  if(greeting.length>500)return json({error:"Greeting must be 500 characters or fewer."},400);
  await env.MALLOW_DB.prepare("INSERT INTO guild_settings (guild_id,prefix,greeting,greetings,mention_only,disabled_commands,updated_at) VALUES (?,?,?,?,?,?,?) ON CONFLICT(guild_id) DO UPDATE SET prefix=excluded.prefix,greeting=excluded.greeting,greetings=excluded.greetings,mention_only=excluded.mention_only,disabled_commands=excluded.disabled_commands,updated_at=excluded.updated_at").bind(guildId,prefix,greeting,body.greetings?1:0,body.mention_only?1:0,JSON.stringify(disabled),new Date().toISOString()).run();
  return json({ok:true,settings:{prefix,greeting,greetings:!!body.greetings,mention_only:!!body.mention_only,disabled_commands:disabled}});
 }

 if(path==="me"&&request.method==="GET"){const s=await session(request,env);if(!s)return json({user:null,settings:defaults,is_owner:false});return json({user:s.user,settings:await settings(env),is_owner:s.user.id===env.BOT_OWNER_ID});}
 if(path==="logout"&&request.method==="POST")return json({ok:true},200,{"set-cookie":cookieHeader("",0)});
 if(path==="settings"&&request.method==="GET"){const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);return json({settings:await settings(env),is_owner:s.user.id===env.BOT_OWNER_ID});}
 if(path==="settings"&&request.method==="PUT"){const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);if(s.user.id!==env.BOT_OWNER_ID)return json({error:"Only the bot owner can change settings."},403);
 let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}const prefix=String(body.prefix??"").trim(),greeting=String(body.greeting??"");if(!prefix||prefix.length>8||/[\r\n]/.test(prefix))return json({error:"Prefix must be 1–8 characters."},400);if(greeting.length>500)return json({error:"Greeting must be 500 characters or fewer."},400);
 await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK (id=1), prefix TEXT NOT NULL, greeting TEXT NOT NULL, greetings INTEGER NOT NULL, mention_only INTEGER NOT NULL, updated_at TEXT NOT NULL)").run();
 await env.MALLOW_DB.prepare("INSERT INTO settings (id,prefix,greeting,greetings,mention_only,updated_at) VALUES (1,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET prefix=excluded.prefix,greeting=excluded.greeting,greetings=excluded.greetings,mention_only=excluded.mention_only,updated_at=excluded.updated_at").bind(prefix,greeting,body.greetings?1:0,body.mention_only?1:0,new Date().toISOString()).run();return json({ok:true,settings:await settings(env)});}

 if(path==="guilds"&&request.method==="GET"){
  const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
  if(!s.user.access_token)return json({error:"Discord permissions are outdated. Sign out and sign in again to grant server-list access."},401);
  const ur=await fetch("https://discord.com/api/users/@me/guilds?with_counts=true",{headers:{authorization:"Bearer "+s.user.access_token}});
  if(!ur.ok)return json({error:ur.status===401?"Discord sign-in expired. Sign out and sign in again.":"Could not load your Discord servers."},ur.status===401?401:502);
  const all=await ur.json();const manageable=all.filter(g=>{try{const p=BigInt(g.permissions||"0");return (p&8n)!==0n||(p&32n)!==0n}catch{return false}});
  if(!env.BOT_TOKEN)return json({error:"Bot presence checking is not configured. Add the Mallow Bot token as the BOT_TOKEN secret in Cloudflare Pages → Settings → Variables and Secrets, then redeploy."},503);
  const botHeaders={authorization:"Bot "+env.BOT_TOKEN};
  const botMeResponse=await fetch("https://discord.com/api/users/@me",{headers:botHeaders});
  if(!botMeResponse.ok){
   console.error("Mallow BOT_TOKEN validation failed",botMeResponse.status);
   return json({error:botMeResponse.status===401?"The configured BOT_TOKEN is invalid or expired. Replace it with the token for the Mallow Bot application in Cloudflare Pages → Settings → Variables and Secrets, then redeploy.":"Discord could not verify BOT_TOKEN. Check the Cloudflare Pages secret and try again."},503);
  }
  const botMe=await botMeResponse.json();
  if(botMe.id!==env.CLIENT_ID)return json({error:"BOT_TOKEN belongs to a different Discord application than CLIENT_ID. Set both to the Mallow Bot application, then redeploy."},503);
  // Check each manageable server directly using the verified bot token.
  const checks=await Promise.all(manageable.map(async g=>{
   try{
    const br=await fetch("https://discord.com/api/guilds/"+g.id,{headers:botHeaders});
    if(br.ok)return [g.id,true];
    if(br.status===403||br.status===404)return [g.id,false];
    console.error("Mallow bot guild check failed",g.id,br.status);
    throw new Error("Discord returned "+br.status+" while checking server access.");
   }catch(e){console.error("Mallow bot guild check errored",g.id,String(e?.message||e).slice(0,160));throw e;}
  })).catch(()=>null);
  if(!checks)return json({error:"Discord could not verify whether Mallow is in your servers. Check the Cloudflare Pages Function logs and try again."},502);
  const botGuildIds=new Set(checks.filter(([,present])=>present).map(([id])=>id));
  const guilds=manageable.map(g=>({id:g.id,name:g.name,icon_url:g.icon?"https://cdn.discordapp.com/icons/"+g.id+"/"+g.icon+".png?size=128":null,member_count:g.approximate_member_count??null,bot_present:botGuildIds.has(g.id)}));
  return json({guilds,bot_status_available:true});
 }
 if(path==="invite"&&request.method==="GET"){
  const s=await session(request,env);if(!s)return Response.redirect("/api/login",302);
  const guildId=url.searchParams.get("guild_id")||"";if(!/^\d{17,20}$/.test(guildId))return json({error:"Invalid Discord server ID."},400);
  const auth=new URL("https://discord.com/oauth2/authorize");auth.searchParams.set("client_id",env.CLIENT_ID);auth.searchParams.set("scope","bot applications.commands");auth.searchParams.set("permissions","0");auth.searchParams.set("guild_id",guildId);auth.searchParams.set("disable_guild_select","true");
  return Response.redirect(auth.toString(),302);
 }
 return json({error:"Not found."},404);
}
export async function onRequest(context) {
 const {request,env}=context;
 // OAuth state is issued as a separate cookie before leaving for Discord.
 const url=new URL(request.url);
 if(url.pathname==="/api/login"&&request.method==="GET"){
  const missing=["CLIENT_ID","SESSION_SECRET","REDIRECT_URI"].filter(k=>!env[k]);if(missing.length)return json({error:"OAuth configuration missing at runtime.",missing},503);
  const state=b64(crypto.getRandomValues(new Uint8Array(24)));const signed=await token({state,exp:Date.now()+10*60*1000},env.SESSION_SECRET);const auth=new URL("https://discord.com/oauth2/authorize");auth.searchParams.set("client_id",env.CLIENT_ID);auth.searchParams.set("redirect_uri",env.REDIRECT_URI);auth.searchParams.set("response_type","code");auth.searchParams.set("scope","identify guilds");auth.searchParams.set("state",state);
  return new Response(null,{status:302,headers:{"location":auth.toString(),"set-cookie":"mallow_oauth_state="+encodeURIComponent(signed)+"; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=600","cache-control":"no-store"}});
 }
 try{return await route(request,env)}catch(e){console.error("Mallow dashboard request failed",{path:url.pathname,method:request.method,errorName:e?.name||"Error",errorMessage:String(e?.message||e).slice(0,300)});return json({error:"Dashboard request failed. Check the Pages Function logs."},500)}
}
// Redeploy after refreshing the production session secret.
// Redeploy after updating the Discord OAuth client secret.
