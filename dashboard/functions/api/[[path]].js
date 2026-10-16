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

 async function refreshDiscordSession(env, s, force=false) {
  const expires=Number(s?.user?.token_expires_at||0);
  if(!s?.user?.refresh_token) return {session:s,refreshed:false};
  if(!force&&expires>Date.now()+60_000) return {session:s,refreshed:false};
  const form=new URLSearchParams({client_id:env.CLIENT_ID,client_secret:env.CLIENT_SECRET,grant_type:"refresh_token",refresh_token:s.user.refresh_token});
  const r=await fetch("https://discord.com/api/oauth2/token",{method:"POST",headers:{"content-type":"application/x-www-form-urlencoded"},body:form});
  if(!r.ok) return {session:s,refreshed:false,refreshFailed:true};
  const d=await r.json();
  return {session:{...s,user:{...s.user,access_token:d.access_token,refresh_token:d.refresh_token||s.user.refresh_token,token_expires_at:Date.now()+Number(d.expires_in||604800)*1000}},refreshed:true};
}
async function cachedUserGuilds(env, s) {
  await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS oauth_guild_cache (user_id TEXT PRIMARY KEY, guilds_json TEXT NOT NULL, expires_at INTEGER NOT NULL)").run();
  const cached=await env.MALLOW_DB.prepare("SELECT guilds_json,expires_at FROM oauth_guild_cache WHERE user_id=?").bind(s.user.id).first();
  if(cached && Number(cached.expires_at)>Date.now()){try{return {guilds:JSON.parse(cached.guilds_json),session:s}}catch{}}
  let active=s;let r=await fetch("https://discord.com/api/users/@me/guilds",{headers:{authorization:"Bearer "+active.user.access_token}});
  if(r.status===401){const refreshed=await refreshDiscordSession(env,active,true);if(refreshed.refreshed){active=refreshed.session;r=await fetch("https://discord.com/api/users/@me/guilds",{headers:{authorization:"Bearer "+active.user.access_token}})}}
  if(!r.ok){const retryAfter=Math.max(1,Number(r.headers.get("Retry-After")||60));return {error:r.status===429?`Discord is rate-limiting requests. Wait about ${Math.ceil(retryAfter)} seconds before trying again.`:`Discord guild list failed (HTTP ${r.status}).`,status:r.status===429?429:(r.status===401?401:502),retryAfter:r.status===429?retryAfter:undefined}}
  const guilds=await r.json();
  await env.MALLOW_DB.prepare("INSERT INTO oauth_guild_cache(user_id,guilds_json,expires_at) VALUES(?,?,?) ON CONFLICT(user_id) DO UPDATE SET guilds_json=excluded.guilds_json,expires_at=excluded.expires_at").bind(active.user.id,JSON.stringify(guilds),Date.now()+5*60*1000).run();
  return {guilds,session:active};
}
async function authorizedGuild(s, env, guildId) {
  if(!/^\d{17,20}$/.test(guildId||"")) return {error:"Select a valid Discord server.",status:400};
  if(!s?.user?.access_token) return {error:"Sign in again to manage server settings.",status:401};
  const result=await cachedUserGuilds(env,s);if(result.error)return result;s=result.session;
  const guild=result.guilds.find(g=>g.id===guildId);
  if(!guild) return {error:"That server is not available to your Discord account.",status:403};
  let permissions=0n;try{permissions=BigInt(guild.permissions||"0")}catch{}
  if((permissions&8n)===0n&&(permissions&32n)===0n) return {error:"You need Administrator or Manage Server permission to configure Mallow here.",status:403};
  if(env.BOT_TOKEN){const br=await fetch("https://discord.com/api/guilds/"+guildId,{headers:{authorization:"Bot "+env.BOT_TOKEN}});if(!br.ok)return {error:"Mallow Bot must be in this server before you can configure it.",status:409};}
  return {guild};
}
 async function ensureGuildSettings(env){
  await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS guild_settings (guild_id TEXT PRIMARY KEY, prefix TEXT NOT NULL DEFAULT '-mb', greeting TEXT NOT NULL DEFAULT 'hey Mallow Bot', greetings INTEGER NOT NULL DEFAULT 1, mention_only INTEGER NOT NULL DEFAULT 1, disabled_commands TEXT NOT NULL DEFAULT '[]', features TEXT NOT NULL DEFAULT '{}', economy_config TEXT NOT NULL DEFAULT '{}', xp_config TEXT NOT NULL DEFAULT '{}', updated_at TEXT NOT NULL)").run();
  for(const sql of [
   "ALTER TABLE guild_settings ADD COLUMN features TEXT NOT NULL DEFAULT '{}'",
   "ALTER TABLE guild_settings ADD COLUMN economy_config TEXT NOT NULL DEFAULT '{}'",
   "ALTER TABLE guild_settings ADD COLUMN xp_config TEXT NOT NULL DEFAULT '{}'",
   "ALTER TABLE guild_settings ADD COLUMN messages_config TEXT NOT NULL DEFAULT '{}'",
   "ALTER TABLE guild_settings ADD COLUMN tickets_config TEXT NOT NULL DEFAULT '{}'"
  ]){try{await env.MALLOW_DB.prepare(sql).run()}catch{}}

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


 if(path==="runtime-config"&&request.method==="GET"){
  const expected=String(env.RUNTIME_CONFIG_SECRET||"");
  const supplied=(request.headers.get("Authorization")||"").replace(/^Bearer\s+/i,"");
  if(!expected||!supplied||supplied!==expected)return json({error:"Unauthorized."},401);
  const guildId=url.searchParams.get("guild_id")||"";
  if(!/^\d{17,20}$/.test(guildId))return json({error:"Invalid server ID."},400);
  await ensureGuildSettings(env);
  const row=await env.MALLOW_DB.prepare("SELECT prefix,greeting,greetings,mention_only,disabled_commands,features,economy_config,xp_config,messages_config,tickets_config FROM guild_settings WHERE guild_id=?").bind(guildId).first();
  let disabled=[];try{disabled=JSON.parse(row?.disabled_commands||"[]");if(!Array.isArray(disabled))disabled=[]}catch{}
  const parse=(v,fallback)=>{try{return {...fallback,...JSON.parse(v||"{}")}}catch{return fallback}};
  const features=parse(row?.features,{economy:true,xp:true,moderation:true,music:true,fun:true,movies:true,utility:true,social:true});
  const economy=parse(row?.economy_config,{starting_money:100,daily_reward:100,work_min:20,work_max:80});
  const xp=parse(row?.xp_config,{min_xp:10,max_xp:20,cooldown_seconds:60,announce_level_up:true});
  const messages=parse(row?.messages_config,{welcome_enabled:false,welcome_channel_id:"",welcome_message:"Welcome {user} to {server}!",goodbye_enabled:false,goodbye_channel_id:"",goodbye_message:"{user} left {server}.",auto_reply_enabled:false,auto_replies:[]});
  const tickets=parse(row?.tickets_config,{enabled:false,category_id:"",support_role_ids:[],transcript_channel_id:"",welcome_message:"Thanks for opening a ticket. A staff member will be with you soon."});
  return json({guild_id:guildId,settings:{prefix:row?.prefix||defaults.prefix,greeting:row?.greeting||defaults.greeting,greetings:row?!!row.greetings:true,mention_only:row?!!row.mention_only:true,disabled_commands:disabled,features,economy_config:economy,xp_config:xp,messages_config:messages,tickets_config:tickets}});
 }

 if(path==="guild-resources"&&request.method==="GET"){
  const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
  const guildId=url.searchParams.get("guild_id")||"";
  const access=await authorizedGuild(s,env,guildId);if(access.error)return json({error:access.error},access.status);
  if(!env.BOT_TOKEN)return json({error:"Bot token is not configured."},503);
  const headers={authorization:"Bot "+env.BOT_TOKEN};
  const [rr,cr]=await Promise.all([
   fetch("https://discord.com/api/guilds/"+guildId+"/roles",{headers}),
   fetch("https://discord.com/api/guilds/"+guildId+"/channels",{headers})
  ]);
  if(!rr.ok||!cr.ok)return json({error:"Could not load server roles and channels. Make sure Mallow is in this server and has permission to view it."},502);
  const [roles,channels]=await Promise.all([rr.json(),cr.json()]);
  return json({roles:roles.filter(r=>r.id!==guildId).map(r=>({id:r.id,name:r.name,position:r.position})).sort((a,b)=>b.position-a.position),channels:channels.filter(ch=>[0,4,5,15].includes(ch.type)).map(ch=>({id:ch.id,name:ch.name,type:ch.type,parent_id:ch.parent_id||null}))});
 }
 if(path==="messages-settings"&&(request.method==="GET"||request.method==="PUT")){
  const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
  const guildId=url.searchParams.get("guild_id")||"";
  const access=await authorizedGuild(s,env,guildId);if(access.error)return json({error:access.error},access.status);
  await ensureGuildSettings(env);
  const row=await env.MALLOW_DB.prepare("SELECT messages_config FROM guild_settings WHERE guild_id=?").bind(guildId).first();
  const fallback={welcome_enabled:false,welcome_channel_id:"",welcome_message:"Welcome {user} to {server}!",goodbye_enabled:false,goodbye_channel_id:"",goodbye_message:"{user} left {server}.",auto_reply_enabled:false,auto_replies:[]};
  const parse=(v)=>{try{return {...fallback,...JSON.parse(v||"{}")}}catch{return fallback}};
  if(request.method==="GET")return json({messages_config:parse(row?.messages_config)});
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const input=body.messages_config&&typeof body.messages_config==="object"?body.messages_config:{};
  const validId=v=>/^\d{17,20}$/.test(String(v||""))?String(v):"";
  const autoReplies=Array.isArray(input.auto_replies)?input.auto_replies.slice(0,30).map(x=>({trigger:String(x?.trigger||"").trim().slice(0,80),response:String(x?.response||"").trim().slice(0,1000),match_type:x?.match_type==="contains"?"contains":"exact"})).filter(x=>x.trigger&&x.response):[];
  const config={welcome_enabled:!!input.welcome_enabled,welcome_channel_id:validId(input.welcome_channel_id),welcome_message:String(input.welcome_message??fallback.welcome_message).slice(0,1500),goodbye_enabled:!!input.goodbye_enabled,goodbye_channel_id:validId(input.goodbye_channel_id),goodbye_message:String(input.goodbye_message??fallback.goodbye_message).slice(0,1500),auto_reply_enabled:!!input.auto_reply_enabled,auto_replies:autoReplies};
  await env.MALLOW_DB.prepare("INSERT INTO guild_settings (guild_id,messages_config,updated_at) VALUES (?,?,?) ON CONFLICT(guild_id) DO UPDATE SET messages_config=excluded.messages_config,updated_at=excluded.updated_at").bind(guildId,JSON.stringify(config),new Date().toISOString()).run();
  return json({ok:true,messages_config:config});
 }
 if(path==="tickets-settings"&&(request.method==="GET"||request.method==="PUT")){
  const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
  const guildId=url.searchParams.get("guild_id")||"";
  const access=await authorizedGuild(s,env,guildId);if(access.error)return json({error:access.error},access.status);
  await ensureGuildSettings(env);
  const row=await env.MALLOW_DB.prepare("SELECT tickets_config FROM guild_settings WHERE guild_id=?").bind(guildId).first();
  const fallback={enabled:false,category_id:"",support_role_ids:[],transcript_channel_id:"",welcome_message:"Thanks for opening a ticket. A staff member will be with you soon."};
  const parse=(v)=>{try{return {...fallback,...JSON.parse(v||"{}")}}catch{return fallback}};
  if(request.method==="GET")return json({tickets_config:parse(row?.tickets_config)});
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const input=body.tickets_config&&typeof body.tickets_config==="object"?body.tickets_config:{};
  const validId=v=>/^\d{17,20}$/.test(String(v||""))?String(v):"";
  const roleIds=Array.isArray(input.support_role_ids)?[...new Set(input.support_role_ids.map(validId).filter(Boolean))].slice(0,25):[];
  const config={enabled:!!input.enabled,category_id:validId(input.category_id),support_role_ids:roleIds,transcript_channel_id:validId(input.transcript_channel_id),welcome_message:String(input.welcome_message??fallback.welcome_message).slice(0,1500)};
  await env.MALLOW_DB.prepare("INSERT INTO guild_settings (guild_id,tickets_config,updated_at) VALUES (?,?,?) ON CONFLICT(guild_id) DO UPDATE SET tickets_config=excluded.tickets_config,updated_at=excluded.updated_at").bind(guildId,JSON.stringify(config),new Date().toISOString()).run();
  return json({ok:true,tickets_config:config});
 }

 if(path==="guild-settings"&&(request.method==="GET"||request.method==="PUT")){
  const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
  const guildId=url.searchParams.get("guild_id")||"";
  const access=await authorizedGuild(s,env,guildId);if(access.error)return json({error:access.error},access.status,access.retryAfter?{"Retry-After":String(access.retryAfter),"cache-control":"no-store"}:{});
  await ensureGuildSettings(env);
  if(request.method==="GET"){
   let row=await env.MALLOW_DB.prepare("SELECT prefix,greeting,greetings,mention_only,disabled_commands,features,economy_config,xp_config,updated_at FROM guild_settings WHERE guild_id=?").bind(guildId).first();
   if(!row)row={prefix:defaults.prefix,greeting:defaults.greeting,greetings:1,mention_only:1,disabled_commands:"[]",features:"{}",economy_config:"{}",xp_config:"{}",updated_at:null};
   let disabled=[];try{disabled=JSON.parse(row.disabled_commands||"[]");if(!Array.isArray(disabled))disabled=[]}catch{}
   const parse=(v,fallback)=>{try{return {...fallback,...JSON.parse(v||"{}")}}catch{return fallback}};
   return json({guild:{id:guildId,name:access.guild.name,icon:access.guild.icon||null},settings:{prefix:row.prefix,greeting:row.greeting,greetings:!!row.greetings,mention_only:!!row.mention_only,disabled_commands:disabled,features:parse(row.features,{economy:true,xp:true,moderation:true,music:true,fun:true,movies:true,utility:true,social:true}),economy_config:parse(row.economy_config,{starting_money:100,daily_reward:100,work_min:20,work_max:80,currency_symbol:"M$",multiplier:1}),xp_config:parse(row.xp_config,{min_xp:10,max_xp:20,cooldown_seconds:60,announce_level_up:true}),updated_at:row.updated_at}});
  }
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const prefix=String(body.prefix??"-mb").trim(),greeting=String(body.greeting??defaults.greeting);
  const disabled=Array.isArray(body.disabled_commands)?[...new Set(body.disabled_commands.map(x=>String(x).trim()).filter(x=>/^[a-z0-9_-]{1,32}$/i.test(x)))].slice(0,200):[];
  if(!prefix||prefix.length>8||/[\\r\\n\\s]/.test(prefix))return json({error:"Prefix must be 1–8 characters with no spaces or line breaks."},400);
  if(greeting.length>500)return json({error:"Greeting must be 500 characters or fewer."},400);
  const featureDefaults={economy:true,xp:true,moderation:true,music:true,fun:true,movies:true,utility:true,social:true};
  const featureInput=body.features&&typeof body.features==="object"?body.features:{};
  const features=Object.fromEntries(Object.keys(featureDefaults).map(k=>[k,featureInput[k]===undefined?true:!!featureInput[k]]));
  const clamp=(v,min,max,fallback)=>Number.isFinite(Number(v))?Math.max(min,Math.min(max,Math.round(Number(v)))):fallback;
  const ec=body.economy_config||{},xc=body.xp_config||{};
  const economy={starting_money:clamp(ec.starting_money,0,100000000,100),daily_reward:clamp(ec.daily_reward,0,1000000,100),work_min:clamp(ec.work_min,0,1000000,20),work_max:clamp(ec.work_max,0,1000000,80),currency_symbol:String(ec.currency_symbol??"M$").trim().slice(0,8)||"M$",multiplier:Number.isFinite(Number(ec.multiplier))?Math.max(0,Math.min(100,Number(ec.multiplier))):1};
  if(economy.work_max<economy.work_min)return json({error:"Maximum work reward must be at least the minimum."},400);
  const xp={min_xp:clamp(xc.min_xp,0,10000,10),max_xp:clamp(xc.max_xp,0,10000,20),cooldown_seconds:clamp(xc.cooldown_seconds,0,86400,60),announce_level_up:xc.announce_level_up!==false};
  if(xp.max_xp<xp.min_xp)return json({error:"Maximum XP must be at least the minimum."},400);
  await env.MALLOW_DB.prepare("INSERT INTO guild_settings (guild_id,prefix,greeting,greetings,mention_only,disabled_commands,features,economy_config,xp_config,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?) ON CONFLICT(guild_id) DO UPDATE SET prefix=excluded.prefix,greeting=excluded.greeting,greetings=excluded.greetings,mention_only=excluded.mention_only,disabled_commands=excluded.disabled_commands,features=excluded.features,economy_config=excluded.economy_config,xp_config=excluded.xp_config,updated_at=excluded.updated_at").bind(guildId,prefix,greeting,body.greetings?1:0,body.mention_only?1:0,JSON.stringify(disabled),JSON.stringify(features),JSON.stringify(economy),JSON.stringify(xp),new Date().toISOString()).run();
  return json({ok:true,settings:{prefix,greeting,greetings:!!body.greetings,mention_only:!!body.mention_only,disabled_commands:disabled,features,economy_config:economy,xp_config:xp}});
 }

 if(path==="developers"&&request.method==="GET"){
  const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
  await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS dashboard_developers (discord_id TEXT PRIMARY KEY, added_by TEXT NOT NULL, added_at TEXT NOT NULL)").run();
  const owner=s.user.id===String(env.BOT_OWNER_ID);
  const listed=owner?true:!!(await env.MALLOW_DB.prepare("SELECT discord_id FROM dashboard_developers WHERE discord_id=?").bind(s.user.id).first());
  if(!listed)return json({error:"Developer access required.",is_owner:false,is_developer:false},403);
  const rows=await env.MALLOW_DB.prepare("SELECT discord_id,added_by,added_at FROM dashboard_developers ORDER BY added_at DESC").all();
  return json({ok:true,is_owner:owner,is_developer:true,owner_id:String(env.BOT_OWNER_ID),developers:rows.results||[]});
 }
 if(path==="developers"&&request.method==="POST"){
  const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
  if(s.user.id!==String(env.BOT_OWNER_ID))return json({error:"Only the bot owner can add developers."},403);
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const id=String(body.discord_id||"").trim();if(!/^\d{17,20}$/.test(id))return json({error:"Enter a valid 17–20 digit Discord user ID."},400);
  if(id===String(env.BOT_OWNER_ID))return json({error:"The owner already has full access and does not need to be added."},400);
  await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS dashboard_developers (discord_id TEXT PRIMARY KEY, added_by TEXT NOT NULL, added_at TEXT NOT NULL)").run();
  await env.MALLOW_DB.prepare("INSERT INTO dashboard_developers(discord_id,added_by,added_at) VALUES(?,?,?) ON CONFLICT(discord_id) DO NOTHING").bind(id,s.user.id,new Date().toISOString()).run();
  return json({ok:true,discord_id:id});
 }
 if(path==="developers"&&request.method==="DELETE"){
  const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
  if(s.user.id!==String(env.BOT_OWNER_ID))return json({error:"Only the bot owner can remove developers."},403);
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const id=String(body.discord_id||"").trim();if(!/^\d{17,20}$/.test(id))return json({error:"Invalid Discord user ID."},400);
  if(id===String(env.BOT_OWNER_ID))return json({error:"The owner cannot be removed."},400);
  await env.MALLOW_DB.prepare("DELETE FROM dashboard_developers WHERE discord_id=?").bind(id).run();
  return json({ok:true,discord_id:id});
 }
 if(path==="me"&&request.method==="GET"){const s=await session(request,env);if(!s)return json({user:null,settings:defaults,is_owner:false});return json({user:s.user,settings:await settings(env),is_owner:s.user.id===env.BOT_OWNER_ID});}
 if(path==="logout"&&request.method==="POST")return json({ok:true},200,{"set-cookie":cookieHeader("",0)});
 if(path==="settings"&&request.method==="GET"){const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);return json({settings:await settings(env),is_owner:s.user.id===env.BOT_OWNER_ID});}
 if(path==="settings"&&request.method==="PUT"){const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);if(s.user.id!==env.BOT_OWNER_ID)return json({error:"Only the bot owner can change settings."},403);
 let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}const prefix=String(body.prefix??"").trim(),greeting=String(body.greeting??"");if(!prefix||prefix.length>8||/[\r\n]/.test(prefix))return json({error:"Prefix must be 1–8 characters."},400);if(greeting.length>500)return json({error:"Greeting must be 500 characters or fewer."},400);
 await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK (id=1), prefix TEXT NOT NULL, greeting TEXT NOT NULL, greetings INTEGER NOT NULL, mention_only INTEGER NOT NULL, updated_at TEXT NOT NULL)").run();
 await env.MALLOW_DB.prepare("INSERT INTO settings (id,prefix,greeting,greetings,mention_only,updated_at) VALUES (1,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET prefix=excluded.prefix,greeting=excluded.greeting,greetings=excluded.greetings,mention_only=excluded.mention_only,updated_at=excluded.updated_at").bind(prefix,greeting,body.greetings?1:0,body.mention_only?1:0,new Date().toISOString()).run();return json({ok:true,settings:await settings(env)});}

 if(path==="guilds"&&request.method==="GET"){
  let s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
  if(!s.user.access_token)return json({error:"Discord permissions are missing. Sign out and sign in again to grant server-list access."},401);
  const cached=await cachedUserGuilds(env,s);if(cached.error)return json({error:cached.error,retry_after:cached.retryAfter||null},cached.status,cached.retryAfter?{"Retry-After":String(cached.retryAfter),"cache-control":"no-store"}:{});s=cached.session;
  const all=cached.guilds;const manageable=all.filter(g=>{try{const p=BigInt(g.permissions||"0");return (p&8n)!==0n||(p&32n)!==0n}catch{return false}});
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
  const response=json({guilds,bot_status_available:true});if(s.user.token_expires_at){const signed=await token(s,env.SESSION_SECRET);response.headers.append("set-cookie",cookieHeader(signed,7*24*60*60))}return response;
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
