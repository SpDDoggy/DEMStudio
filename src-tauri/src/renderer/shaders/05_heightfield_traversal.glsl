void ray_vs_aabb(vec3 o,vec3 inv,inout vec2 e,vec3 bmin,vec3 bmax){vec3 s=inv*(bmin-o);vec3 t=inv*(bmax-o);vec3 a=min(s,t);vec3 b=max(s,t);e.x=max(max(e.x,a.x),max(a.y,a.z));e.y=min(min(e.y>=0.0?e.y:b.x,b.x),min(b.y,b.z));}vec2 ray_vs_aabb_through(vec3 o,vec3 inv,vec3 bmin,vec3 bmax){vec3 s=inv*(bmin-o);vec3 t=inv*(bmax-o);vec3 a=min(s,t);vec3 b=max(s,t);return vec2(max(a.x,max(a.y,a.z)),min(b.x,min(b.y,b.z)));}vec2 ray_vs_aabb_ia(vec3 o,vec3 inv,vec2 e,inout vec3 n,vec3 bmin,vec3 bmax){vec3 s=inv*(bmin-o);vec3 t=inv*(bmax-o);vec3 a=min(s,t);vec3 b=max(s,t);float ix=max(max(e.x,a.x),max(a.y,a.z));float iy=min(min(e.y>=0.0?e.y:b.x,b.x),min(b.y,b.z));{n=step(ix,a);n.yz*=1.0-n.x;n.z*=1.0-n.y;n*=-sign(inv);}return vec2(ix,iy);}float ray_vs_sphere_exit(vec3 p,vec3 dir,float r){float b=dot(p,dir);float c=dot(p,p)-r*r;float d=sqrt(max(b*b-c,0.0));return max(-b+d,0.0);}bool ray_vs_sphere(vec3 p,vec3 dir,float r,out vec2 e){float b=dot(p,dir);float c=dot(p,p)-r*r;float d=b*b-c;if(d<0.0){return false;}d=sqrt(d);e=vec2(-d,d)-b;return true;}vec3 ray_vs_plane_ia(vec3 o,vec3 dir,vec4 plane,vec2 e){float d=plane_dist(plane,o);float r=dot(dir,plane.xyz);if(r==0.0){return vec3(d<=0.0?e:e.yx,-1.0);}d/=-r;return vec3(mix(vec2(max(e.x,d),e.y),vec2(e.x,min(e.y,d)),step(0.0,r)),d);}
#ifdef OPT_GROUND
uniform vec4 ground;void ray_vs_ground(vec3 o,vec3 dir,vec2 e,inout float out_d,inout vec3 out_n,inout vec2 out_id){if(dir.z>=0.0){return;}float t=(0.0-o.z)/dir.z;if((e.y>=0.0&&t>=e.y)||t<e.x){return;}out_d=t;out_n=vec3(0.0,0.0,1.0);out_id=vec2(ID_AIR,ID_GROUND);}float ray_vs_ground_shadow(vec3 o,vec3 dir,vec2 e){if(dir.z>=0.0){return 1.0;}float t=(0.0-o.z)/dir.z;if((e.y>=0.0&&t>=e.y)||t<e.x){return 1.0;}return 0.0;}
#endif
uniform vec3 vol_bound[2];uniform vec3 air_bound[2];const float BOUND_BORDER=1.0;uniform sampler2D tex_color;uniform vec2 tex_color_inv_size;uniform vec4 K_diffuse,K_ground;uniform vec2 cmap_offset,cmap_scale;uniform float cmap_gamma;vec3 color(float id,vec3 p){vec4 base=(is_map(id)?K_diffuse:K_ground);vec4 color=texture2D(tex_color,(p.xy-cmap_offset)/cmap_scale*tex_color_inv_size);if(cmap_gamma!=1.0){color.xyz=pow(color.xyz,vec3(cmap_gamma));}color*=(1.0-base.w);return base.xyz*(1.0-color.w)+color.xyz;}vec4 material(float id){return vec4(0.0);}uniform sampler2D mipmap;uniform vec2 mipmap_tex_inv_size;uniform vec4 mipmap_size;uniform vec2 mipmap_lod;uniform float mipmap_step;float heightLod(vec2 i_pos,float level){float h=texture2DLod(mipmap,(i_pos+0.5)*mipmap_tex_inv_size,level).x;h=h*mipmap_size.z;
#ifdef OPT_LEVEL
h=ceil(h/mipmap_step)*mipmap_step;
#endif
return h+mipmap_size.w;}
#ifdef PRIM_SURF
float bilinear(vec4 v,vec2 p){return mix(mix(v.x,v.y,p.x),mix(v.z,v.w,p.x),p.y);}vec2 bilinear_grad(vec4 v,vec2 p){return mix(v.yz-v.x,v.w-v.zy,p.yx);}float bi_delta(vec3 p,vec4 h,vec2 i_pos,float inv){return p.z-bilinear(h,(p.xy-i_pos)*inv);}float quad(vec3 coeff,float e){return coeff.x+(coeff.y+coeff.z*e)*e;}void bi_interval(vec3 o,vec3 dir,inout vec2 e,out float s0,out float s1,vec2 i_pos,vec4 h,float inv){float d0=bi_delta(o+dir*e.x,h,i_pos,inv);float d1=bi_delta(o+dir*e.y,h,i_pos,inv);float d2=bi_delta(o+dir*(e.x+e.x-e.y),h,i_pos,inv);s0=sign(d0);s1=sign(d1);if(s0<0.0){s1=-1.0;return;}float A=d0;float B=(d1-d2)*0.5;float C=d1-A-B;vec3 coeff=vec3(A,B,C);if(B*(B+C+C)<0.0){float mid=-B/(C+C);float sm=sign(quad(coeff,mid));if(sm<0.0){e.y=mix(e.x,e.y,mid);s1=sm;}else{e.x=mix(e.x,e.y,mid);s0=sm;}}}bool ray_vs_prim(vec3 o,vec3 dir,inout vec2 e,inout vec3 out_n,vec2 i_pos,vec4 h,float scale,inout vec3 target){float inv=1.0/scale;float s0,s1;bi_interval(o,dir,e,s0,s1,i_pos,h,inv);if(s0<0.0){return true;}if(s1>=0.0){return false;}for(int i=0;i<15;++i){float mid=(e.x+e.y)*0.5;float sm=sign(bi_delta(o+dir*mid,h,i_pos,inv));e=mix(vec2(e.x,mid),vec2(mid,e.y),step(0.0,sm));}out_n=normalize(vec3(-bilinear_grad(h,(o.xy+dir.xy*e.x-i_pos)*inv),scale));target=o+dir*e.x;return true;}float ray_vs_prim_shadow(vec3 o,vec3 dir,vec2 e,vec2 i_pos,vec4 h,float scale){float inv=1.0/scale;float s0,s1;bi_interval(o,dir,e,s0,s1,i_pos,h,inv);return step(0.0,min(s0,s1));}
#endif
#ifdef PRIM_POLY
void gen_planes(vec4 h,float scale,out vec4 plane0,out vec4 plane1,out float s){vec3 p0=vec3(0.0,0.0,h.x);vec3 p1=vec3(scale,scale,h.w);plane0.xyz=(vec3(h.x-h.yz,scale));plane0.w=-dot(plane0.xyz,p0);plane1.xyz=(vec3(h.zy-h.w,scale));plane1.w=-dot(plane1.xyz,p1);s=step(0.0,plane_dist(plane0,p1))*2.0-1.0;}float plane_i(vec3 o,vec3 dir,vec3 pos,vec4 plane,float e){vec2 f=vec2(e-1.0,e);for(int i=0;i<12;++i){float mid=(f.x+f.y)*0.5;float s=sign(plane_dist(plane,o+dir*mid-pos));f=mix(vec2(f.x,mid),vec2(mid,f.y),step(0.0,s));}return f.x;}bool ray_vs_prim(vec3 o,vec3 dir,inout vec2 out_e,inout vec3 out_n,vec2 cell,vec4 h,float scale,inout vec3 target){float s;vec4 plane0,plane1;gen_planes(h,scale,plane0,plane1,s);vec3 pos=vec3(cell,0.0);vec3 e0=ray_vs_plane_ia(o-pos,dir,plane0,out_e);e0.xy=ia_empty(e0.xy,out_e);vec3 e1=ray_vs_plane_ia(o-pos,dir,plane1,out_e);e1.xy=ia_empty(e1.xy,out_e);out_e=s*ia_union(s*e0.xy,s*e1.xy);if(out_e.x>=out_e.y){return false;}if(out_e.x==e0.z){out_n=normalize(plane0.xyz);out_e.x=plane_i(o,dir,pos,plane0,out_e.x);target=o+dir*out_e.x;}else if(out_e.x==e1.z){out_n=normalize(plane1.xyz);out_e.x=plane_i(o,dir,pos,plane1,out_e.x);target=o+dir*out_e.x;}return true;}float ray_vs_prim_shadow(vec3 o,vec3 dir,vec2 out_e,vec2 cell,vec4 h,float scale){float s;vec4 plane0,plane1;gen_planes(h,scale,plane0,plane1,s);vec3 pos=vec3(cell,0.0);vec3 e0=ray_vs_plane_ia(o-pos,dir,plane0,out_e);e0.xy=ia_empty(e0.xy,out_e);vec3 e1=ray_vs_plane_ia(o-pos,dir,plane1,out_e);e1.xy=ia_empty(e1.xy,out_e);out_e=s*ia_union(s*e0.xy,s*e1.xy);return step(out_e.y,out_e.x);}
#endif
#ifdef OPT_GRID
vec4 grid_line(vec2 id,vec3 v,vec3 n){if(is_empty(id.y)||(is_map(id.y)&&parm_grid.z<=0.0)||(is_ground(id.y)&&parm_grid.w<=0.0)){return vec4(0.0);}v.z-=mipmap_size.w;vec3 space=vec3(parm_grid.yy,mipmap_step);vec3 qv=floor(v/space+0.5)*space;
#ifdef OPT_LEVEL
if(parm_grid.z<=1.0){qv.z=-(parm_grid.x+parm_grid.x);}
#endif
qv=mix(qv,-vec3(parm_grid.x+parm_grid.x),step(1.0-1e-6,abs(n)));vec3 c=abs(qv-v);float w_grid=step(parm_grid.x,min3(c));return vec4(K_grid*(1.0-w_grid),1.0-w_grid);}
#endif
void ray_vs_vox(vec3 o,vec3 dir,vec2 e,inout float out_d,inout vec3 out_n,inout vec2 out_id){vec3 inv=1.0/dir;vec3 i_next=step(0.0,inv);vec3 i_sign=i_next*2.0-1.0;vec3 n=vec3(0.0);e=ray_vs_aabb_ia(o,inv,e,n,vol_bound[0],vol_bound[1]);if(e.x>=e.y){return;}vec3 target=mix(vol_bound[1],vol_bound[0],i_next);vec2 i_pos=clamp(floor(o+dir*e.x).xy,vec2(0.0),mipmap_size.xy-1.0);bool hit=false;float lod=mipmap_lod.y;for(int i=0;i<100000&&e.x<e.y;++i){lod=clamp(lod,mipmap_lod.x,mipmap_lod.y);float scale=exp2(lod);vec2 i_cell=floor(i_pos/scale)*scale;vec2 i_end=i_cell+i_next.xy*scale;vec2 t=(i_end-o.xy)*inv.xy;float end=min(min2(t),e.y);float z0=o.z+dir.z*e.x;float z1=o.z+dir.z*end;
#if!defined(PRIM_CUBE)
if(lod<=mipmap_lod.x){vec4 H=vec4(heightLod(i_pos,lod),heightLod(i_pos+vec2(scale,0.0),lod),heightLod(i_pos+vec2(0.0,scale),lod),heightLod(i_pos+vec2(scale,scale),lod));float h=max4(H);if(h>0.0&&min(z0,z1)<=h){vec2 local_e=vec2(e.x,clamp(end,e.x,e.y));if(ray_vs_prim(o,dir,local_e,n,i_cell,H,scale,target)){e.x=local_e.x;hit=true;break;}}}else
#endif
{float h=heightLod(i_pos,lod);if(h>0.0){if(z0<=h){
#if defined(PRIM_CUBE)
if(lod<=mipmap_lod.x){hit=true;break;}
#endif
lod-=1.0;continue;}if(z1<=h){e.x=(h-o.z)*inv.z;n=vec3(0.0,0.0,1.0);target=vec3(0.0,0.0,h);i_pos=clamp(floor(o.xy+dir.xy*e.x),i_cell,i_cell+scale-1.0);
#if defined(PRIM_CUBE)
if(lod<=mipmap_lod.x){hit=true;break;}
#endif
lod-=1.0;continue;}}}e.x=end;n=-i_sign*vec3(minMask2(t,end),0.0);target=vec3(i_end,0.0);vec2 next_pos=clamp(floor(o.xy+dir.xy*e.x),i_cell,i_cell+scale-1.0)-n.xy;lod+=(floor(i_pos/(scale+scale))!=floor(next_pos/(scale+scale))?1.0:0.0);i_pos=next_pos;}if(hit){out_id.y=ID_MAP;out_n=n;out_d=e.x;if(max3(abs(out_n))==1.0){plane_divide(dot(o,out_n),dot(target,out_n),dot(dir,out_n),out_d);}}}float ray_vs_vox_shadow(vec3 o,vec3 dir,vec2 e){vec3 inv=1.0/dir;vec3 i_next=step(0.0,inv);vec3 i_sign=i_next*2.0-1.0;ray_vs_aabb(o,inv,e,vol_bound[0],vol_bound[1]);if(e.x>=e.y){return 1.0;}vec2 i_pos=clamp(floor(o+dir*e.x).xy,vec2(0.0),mipmap_size.xy-1.0);float att=1.0;float lod=mipmap_lod.y;for(int i=0;i<100000&&e.x<e.y;++i){lod=clamp(lod,mipmap_lod.x,mipmap_lod.y);float scale=exp2(lod);vec2 i_cell=floor(i_pos/scale)*scale;vec2 i_end=i_cell+i_next.xy*scale;vec2 t=(i_end-o.xy)*inv.xy;float end=min(min2(t),e.y);float z0=o.z+dir.z*e.x;float z1=o.z+dir.z*end;
#if!defined(PRIM_CUBE)
if(lod<=mipmap_lod.x){vec4 H=vec4(heightLod(i_pos,lod),heightLod(i_pos+vec2(scale,0.0),lod),heightLod(i_pos+vec2(0.0,scale),lod),heightLod(i_pos+vec2(scale,scale),lod));float h=max4(H);if(h>0.0&&min(z0,z1)<=h){vec2 local_e=vec2(e.x,clamp(end,e.x,e.y));att=ray_vs_prim_shadow(o,dir,local_e,i_cell,H,scale);if(att<=0.0){break;}}}else
#endif
{float h=heightLod(i_pos,lod);if(h>0.0){if(z0<=h){
#if defined(PRIM_CUBE)
if(lod<=mipmap_lod.x){att=0.0;break;}
#endif
lod-=1.0;continue;}if(z1<=h){e.x=(h-o.z)*inv.z;i_pos=clamp(floor(o.xy+dir.xy*e.x),i_cell,i_cell+scale-1.0);
#if defined(PRIM_CUBE)
if(lod<=mipmap_lod.x){att=0.0;break;}
#endif
lod-=1.0;continue;}}}e.x=end;vec3 n=-i_sign*vec3(minMask2(t,end),0.0);vec2 next_pos=clamp(floor(o.xy+dir.xy*e.x),i_cell,i_cell+scale-1.0)-n.xy;lod+=(floor(i_pos/(scale+scale))!=floor(next_pos/(scale+scale))?1.0:0.0);i_pos=next_pos;}return att;}const float OFFSET=0.0;vec2 ray_vs_world(vec3 o,vec3 dir,inout float d,inout vec3 n){vec2 id=vec2(ID_AIR);n=vec3(0.0);ray_vs_vox(o,dir,vec2(OFFSET,d),d,n,id);
#ifdef OPT_GROUND
ray_vs_ground(o,dir,vec2(OFFSET,d),d,n,id);
#endif
return id;}float expo_fog_dist(float base,float decay,vec3 o,vec3 dir,float amount){o.z-=air_bound[0].z;float density=base*exp(-decay*o.z);if(abs(dir.z)<1e-5||decay==0.0){return amount/density;}float k=decay*dir.z;return abs(log(abs(amount*-k/density+1.0))/-k);}float expo_fog_amount(float base,float decay,vec3 o,vec3 dir,float d){o.z-=air_bound[0].z;float density=base*exp(-decay*o.z);if(abs(dir.z)<1e-5||decay==0.0){return density*d;}float k=decay*dir.z;return max((density*exp(-k*d)-density)/-k,0.0);}float expo_fog_att(vec3 o,vec3 dir,float d){vec2 e=vec2(0.0,d<0.0?1e6:d);ray_vs_aabb(o,1.0/dir,e,air_bound[0],air_bound[1]);if(e.x>=e.y){return 1.0;}d=e.y-e.x;float amount=expo_fog_amount(air_et,air_ea,o+dir*e.x,dir,d);return exp(-amount);}float shadow(vec3 o,vec3 dir,float d){if(d>0.0){d=max(d-MIN,0.0);}
#ifdef OPT_GROUND
{float att=ray_vs_ground_shadow(o,dir,vec2(OFFSET,d));if(att<=0.0){return 0.0;}}
#endif
{float att=ray_vs_vox_shadow(o,dir,vec2(OFFSET,d));if(att<=0.0){return 0.0;}}
#ifdef OPT_FOG
return expo_fog_att(o,dir,d);
#else
return 1.0;
#endif
}
#ifdef OPT_ORTH
void CorrectDepth(inout vec3 o,vec3 dir){correct_depth=0.0;bool hit=false;
#ifdef OPT_GROUND
if(dir.z<0.0){correct_depth=(ground.z+1.0-o.z)/dir.z;hit=true;}
#endif
{vec3 bmin=vol_bound[0];vec3 bmax=vol_bound[1];
#ifdef OPT_FOG
bmin=min(bmin,air_bound[0]);bmax=max(bmax,air_bound[1]);
#endif
vec2 e=ray_vs_aabb_through(o,1.0/dir,bmin-BOUND_BORDER,bmax+BOUND_BORDER);if(e.x<e.y&&(!hit||e.x<correct_depth)){correct_depth=e.x;}}o+=dir*correct_depth;}
#endif

