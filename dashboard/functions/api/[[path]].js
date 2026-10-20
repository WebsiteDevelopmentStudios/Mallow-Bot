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

async function sha256Hex(value) {
 const digest=await crypto.subtle.digest("SHA-256",new TextEncoder().encode(value));
 return Array.from(new Uint8Array(digest)).map(b=>b.toString(16).padStart(2,"0")).join("");
}
async function ensureUnfilteredKeyTable(env) {
 await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS unfiltered_api_keys (id TEXT PRIMARY KEY, key_hash TEXT UNIQUE NOT NULL, discord_id TEXT NOT NULL, created_at TEXT NOT NULL, revoked_at TEXT, last_used_at TEXT)").run();
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
  const access=await authorizedGuild(s,env,guildId);if(access.error)return json({error:access.error},access.status);const premium=await premiumFor(env,s.user.id);if(!premium.active&&s.user.id!==String(env.BOT_OWNER_ID))return json({error:"An active seven-day Premium subscription is required to configure roles and channels."},402);
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
  const access=await authorizedGuild(s,env,guildId);if(access.error)return json({error:access.error},access.status);const premium=await premiumFor(env,s.user.id);if(!premium.active&&s.user.id!==String(env.BOT_OWNER_ID))return json({error:"An active seven-day Premium subscription is required to configure roles and channels."},402);
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
  const access=await authorizedGuild(s,env,guildId);if(access.error)return json({error:access.error},access.status);const premium=await premiumFor(env,s.user.id);if(!premium.active&&s.user.id!==String(env.BOT_OWNER_ID))return json({error:"An active seven-day Premium subscription is required to configure roles and channels."},402);
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
   return json({guild:{id:guildId,name:access.guild.name,icon:access.guild.icon||null},settings:{prefix:row.prefix,greeting:row.greeting,greetings:!!row.greetings,mention_only:!!row.mention_only,disabled_commands:disabled,features:parse(row.features,{economy:true,xp:true,moderation:true,music:true,fun:true,movies:true,utility:true,social:true,ai:false}),economy_config:parse(row.economy_config,{starting_money:100,daily_reward:100,work_min:20,work_max:80,currency_symbol:"M$",multiplier:1}),xp_config:parse(row.xp_config,{min_xp:10,max_xp:20,cooldown_seconds:60,announce_level_up:true}),updated_at:row.updated_at}});
  }
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const prefix=String(body.prefix??"-mb").trim(),greeting=String(body.greeting??defaults.greeting);
  const disabled=Array.isArray(body.disabled_commands)?[...new Set(body.disabled_commands.map(x=>String(x).trim()).filter(x=>/^[a-z0-9_-]{1,32}$/i.test(x)))].slice(0,200):[];
  if(!prefix||prefix.length>8||/[\\r\\n\\s]/.test(prefix))return json({error:"Prefix must be 1–8 characters with no spaces or line breaks."},400);
  if(greeting.length>500)return json({error:"Greeting must be 500 characters or fewer."},400);
  const featureDefaults={economy:true,xp:true,moderation:true,music:true,fun:true,movies:true,utility:true,social:true,ai:false};
  const featureInput=body.features&&typeof body.features==="object"?body.features:{};
  const features=Object.fromEntries(Object.keys(featureDefaults).map(k=>[k,featureInput[k]===undefined?featureDefaults[k]:!!featureInput[k]]));
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
 async function ensurePremiumTables(env){
  await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS premium_codes (code TEXT PRIMARY KEY, created_by TEXT NOT NULL, created_at TEXT NOT NULL, redeemed_by TEXT, redeemed_at TEXT, expires_at TEXT)").run();
  await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS premium_subscriptions (discord_id TEXT PRIMARY KEY, starts_at TEXT NOT NULL, expires_at TEXT NOT NULL, source_code TEXT NOT NULL)").run();
  await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS premium_bans (discord_id TEXT PRIMARY KEY, banned_by TEXT NOT NULL, banned_at TEXT NOT NULL, reason TEXT NOT NULL DEFAULT '')").run();
 }
 async function premiumFor(env,id){
  await ensurePremiumTables(env);
  const ban=await env.MALLOW_DB.prepare("SELECT discord_id,reason,banned_at FROM premium_bans WHERE discord_id=?").bind(String(id)).first();
  const row=await env.MALLOW_DB.prepare("SELECT starts_at,expires_at,source_code FROM premium_subscriptions WHERE discord_id=?").bind(String(id)).first();
  return {active:!ban&&!!row&&Date.parse(row.expires_at)>Date.now(),banned:!!ban,ban_reason:ban?.reason||null,banned_at:ban?.banned_at||null,starts_at:row?.starts_at||null,expires_at:row?.expires_at||null,source_code:row?.source_code||null};
 }
 if(path==="premium-admin"&&(request.method==="GET"||request.method==="POST"||request.method==="DELETE")){
  const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
  if(s.user.id!==String(env.BOT_OWNER_ID))return json({error:"Only the owner can manage Premium subscriptions and bans."},403);
  await ensurePremiumTables(env);
  if(request.method==="GET"){
   const subs=await env.MALLOW_DB.prepare("SELECT discord_id,starts_at,expires_at,source_code FROM premium_subscriptions ORDER BY expires_at DESC LIMIT 500").all();
   const bans=await env.MALLOW_DB.prepare("SELECT discord_id,banned_by,banned_at,reason FROM premium_bans ORDER BY banned_at DESC LIMIT 500").all();
   return json({ok:true,subscriptions:subs.results||[],bans:bans.results||[]});
  }
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const id=String(body.discord_id||"").trim();if(!/^\d{17,20}$/.test(id))return json({error:"Enter a valid 17–20 digit Discord user ID."},400);
  if(id===String(env.BOT_OWNER_ID))return json({error:"The owner cannot be banned or have Premium revoked here."},400);
  if(request.method==="POST"){
   const action=String(body.action||"");
   if(action==="grant"){
    const days=Math.max(1,Math.min(3650,Number.parseInt(body.days||"7",10)||7));
    const ban=await env.MALLOW_DB.prepare("SELECT discord_id FROM premium_bans WHERE discord_id=?").bind(id).first();
    if(ban)return json({error:"Unban this user before granting Premium."},409);
    const oldSub=await env.MALLOW_DB.prepare("SELECT starts_at,expires_at FROM premium_subscriptions WHERE discord_id=?").bind(id).first();
    const now=Date.now();const active=oldSub&&Date.parse(oldSub.expires_at)>now;const startsAt=active?oldSub.starts_at:new Date(now).toISOString();const base=active?Date.parse(oldSub.expires_at):now;const expiresAt=new Date(base+days*86400000).toISOString();
    await env.MALLOW_DB.prepare("INSERT INTO premium_subscriptions(discord_id,starts_at,expires_at,source_code) VALUES(?,?,?,?) ON CONFLICT(discord_id) DO UPDATE SET starts_at=excluded.starts_at,expires_at=excluded.expires_at,source_code=excluded.source_code").bind(id,startsAt,expiresAt,"owner:"+s.user.id).run();
    return json({ok:true,discord_id:id,premium:{active:true,starts_at:startsAt,expires_at:expiresAt}});
   }
   if(action==="ban"){
    const reason=String(body.reason||"").trim().slice(0,500);
    await env.MALLOW_DB.prepare("INSERT INTO premium_bans(discord_id,banned_by,banned_at,reason) VALUES(?,?,?,?) ON CONFLICT(discord_id) DO UPDATE SET banned_by=excluded.banned_by,banned_at=excluded.banned_at,reason=excluded.reason").bind(id,s.user.id,new Date().toISOString(),reason).run();
    await env.MALLOW_DB.prepare("DELETE FROM premium_subscriptions WHERE discord_id=?").bind(id).run();
    return json({ok:true,discord_id:id,banned:true});
   }
   if(action==="unban"){await env.MALLOW_DB.prepare("DELETE FROM premium_bans WHERE discord_id=?").bind(id).run();return json({ok:true,discord_id:id,banned:false});}
   if(action==="revoke"){await env.MALLOW_DB.prepare("DELETE FROM premium_subscriptions WHERE discord_id=?").bind(id).run();return json({ok:true,discord_id:id,revoked:true});}
   return json({error:"Unknown action."},400);
  }
  await env.MALLOW_DB.prepare("DELETE FROM premium_subscriptions WHERE discord_id=?").bind(id).run();
  await env.MALLOW_DB.prepare("DELETE FROM premium_bans WHERE discord_id=?").bind(id).run();
  return json({ok:true,discord_id:id,revoked:true,banned:false});
 }
 if(path==="premium-check"&&request.method==="GET"){
  const id=String(url.searchParams.get("discord_id")||"").trim();
  if(!/^\\d{17,20}$/.test(id))return json({error:"Invalid Discord ID."},400);
  const premium=await premiumFor(env,id);
  return json({ok:true,active:premium.active,expires_at:premium.expires_at,banned:premium.banned});
 }
 if(path==="premium-status"&&request.method==="GET"){
  const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
  const premium=await premiumFor(env,s.user.id);
  return json({ok:true,discord_id:s.user.id,is_owner:s.user.id===String(env.BOT_OWNER_ID),premium});
 }
 if(path==="unfiltered-keys"&&(request.method==="GET"||request.method==="POST"||request.method==="DELETE")){
   const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
   const owner=s.user.id===String(env.BOT_OWNER_ID);
   const premium=await premiumFor(env,s.user.id);
   if(!owner&&(!premium.active||premium.banned))return json({error:"Active Premium is required to create or use Zoi Unfiltered API keys."},403);
   await ensureUnfilteredKeyTable(env);
   if(request.method==="GET"){
    const rows=await env.MALLOW_DB.prepare("SELECT id,created_at,last_used_at FROM unfiltered_api_keys WHERE discord_id=? AND revoked_at IS NULL ORDER BY created_at DESC").bind(s.user.id).all();
    return json({ok:true,keys:rows.results||[]});
   }
   if(request.method==="POST"){
    const count=await env.MALLOW_DB.prepare("SELECT COUNT(*) AS count FROM unfiltered_api_keys WHERE discord_id=? AND revoked_at IS NULL").bind(s.user.id).first();
    if(Number(count?.count||0)>=5)return json({error:"You can have up to five active Unfiltered API keys. Revoke one before generating another."},409);
    const raw="mallow_zoiu_"+Array.from(crypto.getRandomValues(new Uint8Array(32))).map(b=>b.toString(16).padStart(2,"0")).join("");
    const id=crypto.randomUUID();
    const createdAt=new Date().toISOString();
    const hash=await sha256Hex(raw);
    await env.MALLOW_DB.prepare("INSERT INTO unfiltered_api_keys(id,key_hash,discord_id,created_at) VALUES(?,?,?,?)").bind(id,hash,s.user.id,createdAt).run();
    return json({ok:true,key:raw,id:id,created_at:createdAt});
   }
   let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
   const id=String(body.id||"").trim();
   if(!id)return json({error:"Select a key to revoke."},400);
   await env.MALLOW_DB.prepare("UPDATE unfiltered_api_keys SET revoked_at=? WHERE id=? AND discord_id=? AND revoked_at IS NULL").bind(new Date().toISOString(),id,s.user.id).run();
   return json({ok:true,revoked:true});
  }
  if(path==="zoi-unfiltered"&&request.method==="POST"){
   const auth=(request.headers.get("Authorization")||"").replace(/^Bearer\s+/i,"").trim();
   if(!auth.startsWith("mallow_zoiu_"))return json({error:"A valid Mallow Zoi Unfiltered API key is required."},401);
   await ensureUnfilteredKeyTable(env);
   const hash=await sha256Hex(auth);
   const row=await env.MALLOW_DB.prepare("SELECT id,discord_id FROM unfiltered_api_keys WHERE key_hash=? AND revoked_at IS NULL").bind(hash).first();
   if(!row)return json({error:"Invalid or revoked API key."},401);
   const premium=await premiumFor(env,row.discord_id);
   if(row.discord_id!==String(env.BOT_OWNER_ID)&&(!premium.active||premium.banned))return json({error:"An active Premium subscription is required to use Zoi Unfiltered."},403);
   if(!env.ZOI_UNFILTERED_UPSTREAM_URL)return json({error:"The Unfiltered model endpoint has not been configured yet."},503);
   let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
   if(!Array.isArray(body.messages)||body.messages.length<1||body.messages.length>20)return json({error:"Provide between one and twenty chat messages."},400);
   const messages=[];
   for(const item of body.messages){
    if(!item||!["system","user","assistant"].includes(item.role)||typeof item.content!=="string")return json({error:"Each message must have a supported role and text content."},400);
    messages.push({role:item.role,content:item.content.slice(0,6000)});
   }
   await env.MALLOW_DB.prepare("UPDATE unfiltered_api_keys SET last_used_at=? WHERE id=?").bind(new Date().toISOString(),row.id).run();
   const headers={"content-type":"application/json"};
   if(env.ZOI_UNFILTERED_UPSTREAM_KEY)headers.authorization="Bearer "+env.ZOI_UNFILTERED_UPSTREAM_KEY;
   const upstream=await fetch(env.ZOI_UNFILTERED_UPSTREAM_URL,{method:"POST",headers:headers,body:JSON.stringify({model:env.ZOI_UNFILTERED_MODEL||"zoi-unfiltered",messages:messages,temperature:Math.max(0,Math.min(2,Number(body.temperature??0.7)||0.7)),max_tokens:Math.max(1,Math.min(2000,Number.parseInt(body.max_tokens??"700",10)||700))})});
   return new Response(upstream.body,{status:upstream.status,headers:{"content-type":upstream.headers.get("content-type")||"application/json","cache-control":"no-store"}});
  }
  if(path==="premium-codes"&&request.method==="GET"){
  const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
  if(s.user.id!==String(env.BOT_OWNER_ID))return json({error:"Only the owner can manage premium codes."},403);
  await ensurePremiumTables(env);
  const rows=await env.MALLOW_DB.prepare("SELECT code,created_at,redeemed_by,redeemed_at,expires_at FROM premium_codes ORDER BY created_at DESC LIMIT 200").all();
  return json({ok:true,codes:rows.results||[]});
 }
 if(path==="premium-codes"&&request.method==="POST"){
  const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
  if(s.user.id!==String(env.BOT_OWNER_ID))return json({error:"Only the owner can create premium codes."},403);
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const count=Math.max(1,Math.min(25,Number.parseInt(body.count||"1",10)||1));
  await ensurePremiumTables(env);const codes=[];
  for(let i=0;i<count;i++){
   const bytes=crypto.getRandomValues(new Uint8Array(12));const code="MALLOW-"+Array.from(bytes,b=>b.toString(16).padStart(2,"0")).join("").toUpperCase();
   await env.MALLOW_DB.prepare("INSERT INTO premium_codes(code,created_by,created_at) VALUES(?,?,?)").bind(code,s.user.id,new Date().toISOString()).run();codes.push(code);
  }
  return json({ok:true,codes});
 }
 if(path==="redeem-premium"&&request.method==="POST"){
  const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const premiumState=await premiumFor(env,s.user.id);if(premiumState.banned)return json({error:"This Discord ID is banned from Premium."},403);
  const code=String(body.code||"").trim().toUpperCase();
  if(!/^MALLOW-[A-F0-9]{24}$/.test(code))return json({error:"Enter a valid premium code."},400);
  await ensurePremiumTables(env);
  const row=await env.MALLOW_DB.prepare("SELECT code,redeemed_by,expires_at FROM premium_codes WHERE code=?").bind(code).first();
  if(!row)return json({error:"That premium code is invalid."},404);
  if(row.redeemed_by)return json({error:"That premium code has already been redeemed."},409);
  const now=Date.now();const current=await premiumFor(env,s.user.id);const start=current.active&&current.expires_at&&Date.parse(current.expires_at)>now?Date.parse(current.expires_at):now;
  const startsAt=current.active?current.starts_at:new Date(now).toISOString();const expiresAt=new Date(start+7*24*60*60*1000).toISOString();
  const result=await env.MALLOW_DB.prepare("UPDATE premium_codes SET redeemed_by=?,redeemed_at=?,expires_at=? WHERE code=? AND redeemed_by IS NULL").bind(s.user.id,new Date(now).toISOString(),expiresAt,code).run();
  if(!result.meta||result.meta.changes===0)return json({error:"That premium code has already been redeemed."},409);
  await env.MALLOW_DB.prepare("INSERT INTO premium_subscriptions(discord_id,starts_at,expires_at,source_code) VALUES(?,?,?,?) ON CONFLICT(discord_id) DO UPDATE SET expires_at=excluded.expires_at,source_code=excluded.source_code").bind(s.user.id,startsAt,expiresAt,code).run();
  return json({ok:true,premium:{active:true,starts_at:startsAt,expires_at:expiresAt}});
 }
 if(path==="channel-messages"&&(request.method==="GET"||request.method==="POST")){
  const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
  const guildId=url.searchParams.get("guild_id")||"";
  const channelId=url.searchParams.get("channel_id")||"";
  const access=await authorizedGuild(s,env,guildId);if(access.error)return json({error:access.error},access.status);
  const premium=await premiumFor(env,s.user.id);if(!premium.active&&s.user.id!==String(env.BOT_OWNER_ID))return json({error:"An active seven-day Premium subscription is required to manage channel messages."},402);
  if(!env.BOT_TOKEN)return json({error:"Bot token is not configured."},503);
  if(!/^\d{17,20}$/.test(channelId))return json({error:"Choose a valid channel."},400);
  const headers={authorization:"Bot "+env.BOT_TOKEN,"content-type":"application/json"};
  const channelRes=await fetch("https://discord.com/api/channels/"+channelId,{headers});
  if(!channelRes.ok)return json({error:"Could not access that channel."},502);
  const channel=await channelRes.json();if(channel.guild_id!==guildId||![0,5,15].includes(channel.type))return json({error:"Choose a text, announcement, or forum channel in this server."},400);
  if(request.method==="GET"){
   const mr=await fetch("https://discord.com/api/channels/"+channelId+"/messages?limit=50",{headers});
   if(!mr.ok)return json({error:"Could not read messages. Check the bot's View Channel and Read Message History permissions."},502);
   const messages=await mr.json();
   return json({ok:true,channel:{id:channel.id,name:channel.name},messages:messages.map(m=>({id:m.id,content:m.content,created_at:m.timestamp,author:{id:m.author?.id,username:m.author?.global_name||m.author?.username,avatar:m.author?.avatar?("https://cdn.discordapp.com/avatars/"+m.author.id+"/"+m.author.avatar+".png?size=64"):null},attachments:(m.attachments||[]).map(a=>({url:a.url,filename:a.filename}))})).reverse()});
  }
  let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
  const content=String(body.content||"").trim();if(!content||content.length>2000)return json({error:"Message must contain 1–2000 characters."},400);
  const sent=await fetch("https://discord.com/api/channels/"+channelId+"/messages",{method:"POST",headers,body:JSON.stringify({content,allowed_mentions:{parse:[]}})});
  if(!sent.ok)return json({error:"Could not send message. Check the bot's Send Messages permission."},502);
  const m=await sent.json();return json({ok:true,message:{id:m.id,content:m.content,created_at:m.timestamp}});
 }
 if(path==="subscriptions"&&request.method==="GET"){
  const s=await session(request,env);if(!s)return json({error:"Sign in with Discord first."},401);
  return json({ok:true,premium:await premiumFor(env,s.user.id)});
 }
 if(path==="me"&&request.method==="GET"){const s=await session(request,env);if(!s)return json({user:null,settings:defaults,is_owner:false});return json({user:s.user,settings:await settings(env),is_owner:s.user.id===env.BOT_OWNER_ID});}
 if(path==="logout"&&request.method==="POST")return json({ok:true},200,{"set-cookie":cookieHeader("",0)});
 if(path==="settings"&&request.method==="GET"){const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);return json({settings:await settings(env),is_owner:s.user.id===env.BOT_OWNER_ID});}
 if(path==="settings"&&request.method==="PUT"){const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);if(s.user.id!==env.BOT_OWNER_ID)return json({error:"Only the bot owner can change settings."},403);
 let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}const prefix=String(body.prefix??"").trim(),greeting=String(body.greeting??"");if(!prefix||prefix.length>8||/[\r\n]/.test(prefix))return json({error:"Prefix must be 1–8 characters."},400);if(greeting.length>500)return json({error:"Greeting must be 500 characters or fewer."},400);
 await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK (id=1), prefix TEXT NOT NULL, greeting TEXT NOT NULL, greetings INTEGER NOT NULL, mention_only INTEGER NOT NULL, updated_at TEXT NOT NULL)").run();
 await env.MALLOW_DB.prepare("INSERT INTO settings (id,prefix,greeting,greetings,mention_only,updated_at) VALUES (1,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET prefix=excluded.prefix,greeting=excluded.greeting,greetings=excluded.greetings,mention_only=excluded.mention_only,updated_at=excluded.updated_at").bind(prefix,greeting,body.greetings?1:0,body.mention_only?1:0,new Date().toISOString()).run();return json({ok:true,settings:await settings(env)});}

 if(path==="developer-guilds"&&request.method==="GET"){
   const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
   if(s.user.id!==String(env.BOT_OWNER_ID))return json({error:"Only the bot owner can view the bot's server list."},403);
   if(!env.BOT_TOKEN)return json({error:"Bot token is not configured."},503);
   const backendUrl=String(env.BOT_BACKEND_URL||"https://d7lyw2yvj2.apps.bot-hosting.cloud").replace(/\/$/,"");
   const response=await fetch(backendUrl+"/owner/guilds",{headers:{authorization:"Bot "+env.BOT_TOKEN}});
   const data=await response.json().catch(()=>({}));
   if(!response.ok)return json({error:data.error||"Could not retrieve Mallow's live server list from the bot service."},response.status===401?503:502);
   const guilds=Array.isArray(data.guilds)?data.guilds.map(g=>({id:g.id,name:g.name,icon_url:g.icon_url||null,member_count:g.member_count??null,bot_present:true})): [];
   return json({guilds,source:"bot"});
  }
 if(path==="guilds"&&request.method==="GET"){
   const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
   const userGuildsResult=await cachedUserGuilds(env,s);
   if(userGuildsResult.error)return json({error:userGuildsResult.error},userGuildsResult.status||502);
   const userGuilds=userGuildsResult.guilds.filter(g=>{let p=0n;try{p=BigInt(g.permissions||"0")}catch{}return (p&8n)!==0n||(p&32n)!==0n;});
   let botGuildIds=new Set();
   if(env.BOT_TOKEN){
    const botMeResponse=await fetch("https://discord.com/api/users/@me",{headers:{authorization:"Bot "+env.BOT_TOKEN}});
    if(!botMeResponse.ok)return json({error:botMeResponse.status===401?"The configured BOT_TOKEN is invalid.":"Discord could not verify BOT_TOKEN."},503);
    const botMe=await botMeResponse.json();
    if(botMe.id!==env.CLIENT_ID)return json({error:"BOT_TOKEN belongs to a different Discord application than CLIENT_ID."},503);
    const backendUrl=String(env.BOT_BACKEND_URL||"https://d7lyw2yvj2.apps.bot-hosting.cloud").replace(/\/$/,"");
    const response=await fetch(backendUrl+"/owner/guilds",{headers:{authorization:"Bot "+env.BOT_TOKEN}});
    const data=await response.json().catch(()=>({}));
    if(!response.ok)return json({error:data.error||"Could not retrieve Mallow's live server list from the bot service."},response.status===401?503:502);
    botGuildIds=new Set((Array.isArray(data.guilds)?data.guilds:[]).map(g=>String(g.id)));
   }
   const guilds=userGuilds.map(g=>({id:g.id,name:g.name,icon_url:g.icon?("https://cdn.discordapp.com/icons/"+g.id+"/"+g.icon+".png?size=128"):null,member_count:null,bot_present:botGuildIds.has(String(g.id))})).sort((a,b)=>Number(b.bot_present)-Number(a.bot_present)||a.name.localeCompare(b.name));
   return json({guilds,bot_status_available:!!env.BOT_TOKEN,source:"discord"});
  }
if(path==="leave"&&request.method==="POST"){
   const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);
   if(s.user.id!==env.BOT_OWNER_ID)return json({error:"Only the bot owner can make Mallow leave a server."},403);
   if(!env.BOT_TOKEN)return json({error:"Bot token is not configured."},503);
   let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}
   const guildId=String(body.guild_id||"");if(!/^\d{17,20}$/.test(guildId))return json({error:"Invalid Discord server ID."},400);
   const botHeaders={authorization:"Bot "+env.BOT_TOKEN,"content-type":"application/json"};
   const backendUrl=String(env.BOT_BACKEND_URL||"https://d7lyw2yvj2.apps.bot-hosting.cloud").replace(/\/$/,"");
   const response=await fetch(backendUrl+"/owner/leave",{method:"POST",headers:botHeaders,body:JSON.stringify({guild_id:guildId})});
   const data=await response.json().catch(()=>({}));
   if(!response.ok)return json({error:data.error||"Mallow could not leave that server."},response.status===401?503:502);
   return json({ok:true});
  }
if(path==="server-invite"&&request.method==="GET"){
  const s=await session(request,env);if(!s)return Response.redirect("/api/login",302);
  if(s.user.id!==env.BOT_OWNER_ID)return json({error:"Only the bot owner can generate server invite links."},403);
  if(!env.BOT_TOKEN)return json({error:"Bot token is not configured."},503);
  const guildId=url.searchParams.get("guild_id")||"";if(!/^\d{17,20}$/.test(guildId))return json({error:"Invalid Discord server ID."},400);
  const headers={authorization:"Bot "+env.BOT_TOKEN};
  const membership=await fetch("https://discord.com/api/guilds/"+guildId,{headers});
  if(membership.status===404)return json({error:"Mallow is not in this server, so it cannot create an invite for you."},409);
  if(!membership.ok)return json({error:"Discord could not verify Mallow's membership in this server."},502);

  const channelsResponse=await fetch("https://discord.com/api/guilds/"+guildId+"/channels",{headers});
  if(!channelsResponse.ok)return json({error:"Could not list server channels to create an invite."},502);
  const channels=await channelsResponse.json();
  const candidates=channels.filter(ch=>ch.type===0||ch.type===5).sort((a,b)=>(a.position||0)-(b.position||0));
  for(const channel of candidates){
   const created=await fetch("https://discord.com/api/channels/"+channel.id+"/invites",{method:"POST",headers:{...headers,"content-type":"application/json"},body:JSON.stringify({max_age:0,max_uses:0,temporary:false,unique:true})});
   if(created.ok){const invite=await created.json();return Response.redirect("https://discord.gg/"+invite.code,302)}
   if(created.status!==403&&created.status!==404){console.error("Discord invite creation failed",guildId,channel.id,created.status)}
  }
  return json({error:"Mallow could not create an invite in this server. Give the bot permission to Create Invite in a text channel, then try again."},502);
 }
 if(path==="invite"&&request.method==="GET"){
  const s=await session(request,env);if(!s)return Response.redirect("/api/login",302);
  const guildId=url.searchParams.get("guild_id")||"";if(!/^\d{17,20}$/.test(guildId))return json({error:"Invalid Discord server ID."},400);
  const auth=new URL("https://discord.com/oauth2/authorize");auth.searchParams.set("client_id",env.CLIENT_ID);auth.searchParams.set("scope","bot applications.commands");auth.searchParams.set("permissions","8");auth.searchParams.set("guild_id",guildId);auth.searchParams.set("disable_guild_select","true");
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
