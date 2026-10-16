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
async function route(request, env) {
 const missing=["CLIENT_ID","CLIENT_SECRET","REDIRECT_URI","SESSION_SECRET","BOT_OWNER_ID","MALLOW_DB"].filter(k=>!env[k]); if(missing.length) return json({error:"Dashboard server configuration is incomplete.",missing},503);
 const url=new URL(request.url); const path=url.pathname.replace(/^\/api\/?/,"");
 if(path==="login"&&request.method==="GET"){const state=b64(crypto.getRandomValues(new Uint8Array(24)));const signed=await token({state,exp:Date.now()+10*60*1000},env.SESSION_SECRET);const auth=new URL("https://discord.com/oauth2/authorize");auth.searchParams.set("client_id",env.CLIENT_ID);auth.searchParams.set("redirect_uri",env.REDIRECT_URI);auth.searchParams.set("response_type","code");auth.searchParams.set("scope","identify");auth.searchParams.set("state",state);return Response.redirect(auth.toString(),302);}
 if(path==="callback"&&request.method==="GET"){const err=url.searchParams.get("error");const code=url.searchParams.get("code");const state=url.searchParams.get("state");if(err||!code||!state)return Response.redirect("/?auth=failed",302);
 // State is also stored in a short-lived cookie to prevent login CSRF.
 const saved=await untoken(cookie(request,"mallow_oauth_state"),env.SESSION_SECRET);if(!saved||saved.state!==state)return Response.redirect("/?auth=state",302);
 const form=new URLSearchParams({client_id:env.CLIENT_ID,client_secret:env.CLIENT_SECRET,grant_type:"authorization_code",code,redirect_uri:env.REDIRECT_URI});
 const tr=await fetch("https://discord.com/api/oauth2/token",{method:"POST",headers:{"content-type":"application/x-www-form-urlencoded"},body:form});if(!tr.ok)return Response.redirect("/?auth=token",302);const td=await tr.json();
 const ur=await fetch("https://discord.com/api/users/@me",{headers:{authorization:"Bearer "+td.access_token}});if(!ur.ok)return Response.redirect("/?auth=user",302);const u=await ur.json();const payload={user:{id:u.id,username:u.username,global_name:u.global_name||u.username,avatar_url:u.avatar?"https://cdn.discordapp.com/avatars/"+u.id+"/"+u.avatar+".png":"https://cdn.discordapp.com/embed/avatars/0.png"},exp:Date.now()+7*24*60*60*1000};
 const signed=await token(payload,env.SESSION_SECRET);const h=new Headers({"location":"/","cache-control":"no-store"});h.append("set-cookie",cookieHeader(signed,7*24*60*60));h.append("set-cookie","mallow_oauth_state=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0");return new Response(null,{status:302,headers:h});
 }
 if(path==="me"&&request.method==="GET"){const s=await session(request,env);if(!s)return json({user:null,settings:defaults,is_owner:false});return json({user:s.user,settings:await settings(env),is_owner:s.user.id===env.BOT_OWNER_ID});}
 if(path==="logout"&&request.method==="POST")return json({ok:true},200,{"set-cookie":cookieHeader("",0)});
 if(path==="settings"&&request.method==="GET"){const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);return json({settings:await settings(env),is_owner:s.user.id===env.BOT_OWNER_ID});}
 if(path==="settings"&&request.method==="PUT"){const s=await session(request,env);if(!s)return json({error:"Sign in required."},401);if(s.user.id!==env.BOT_OWNER_ID)return json({error:"Only the bot owner can change settings."},403);
 let body;try{body=await request.json()}catch{return json({error:"Invalid JSON."},400)}const prefix=String(body.prefix??"").trim(),greeting=String(body.greeting??"");if(!prefix||prefix.length>8||/[\r\n]/.test(prefix))return json({error:"Prefix must be 1–8 characters."},400);if(greeting.length>500)return json({error:"Greeting must be 500 characters or fewer."},400);
 await env.MALLOW_DB.prepare("CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK (id=1), prefix TEXT NOT NULL, greeting TEXT NOT NULL, greetings INTEGER NOT NULL, mention_only INTEGER NOT NULL, updated_at TEXT NOT NULL)").run();
 await env.MALLOW_DB.prepare("INSERT INTO settings (id,prefix,greeting,greetings,mention_only,updated_at) VALUES (1,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET prefix=excluded.prefix,greeting=excluded.greeting,greetings=excluded.greetings,mention_only=excluded.mention_only,updated_at=excluded.updated_at").bind(prefix,greeting,body.greetings?1:0,body.mention_only?1:0,new Date().toISOString()).run();return json({ok:true,settings:await settings(env)});}
 return json({error:"Not found."},404);
}
export async function onRequest(context) {
 const {request,env}=context;
 // OAuth state is issued as a separate cookie before leaving for Discord.
 const url=new URL(request.url);
 if(url.pathname==="/api/login"&&request.method==="GET"){
  const missing=["CLIENT_ID","SESSION_SECRET","REDIRECT_URI"].filter(k=>!env[k]);if(missing.length)return json({error:"OAuth configuration missing at runtime.",missing},503);
  const state=b64(crypto.getRandomValues(new Uint8Array(24)));const signed=await token({state,exp:Date.now()+10*60*1000},env.SESSION_SECRET);const auth=new URL("https://discord.com/oauth2/authorize");auth.searchParams.set("client_id",env.CLIENT_ID);auth.searchParams.set("redirect_uri",env.REDIRECT_URI);auth.searchParams.set("response_type","code");auth.searchParams.set("scope","identify");auth.searchParams.set("state",state);
  return new Response(null,{status:302,headers:{"location":auth.toString(),"set-cookie":"mallow_oauth_state="+encodeURIComponent(signed)+"; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=600","cache-control":"no-store"}});
 }
 try{return await route(request,env)}catch(e){console.error("Mallow dashboard request failed",{path:url.pathname,method:request.method,errorName:e?.name||"Error",errorMessage:String(e?.message||e).slice(0,300)});return json({error:"Dashboard request failed. Check the Pages Function logs."},500)}
}
// Redeploy after refreshing the production session secret.
// Redeploy after updating the Discord OAuth client secret.
