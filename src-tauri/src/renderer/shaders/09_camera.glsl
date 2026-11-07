uniform vec3 view_origin;uniform mat3 view_rot;uniform float view_fov;
#ifdef OPT_DOF
uniform vec4 dof_lens;uniform vec2 dof_rand;
#endif
#ifdef OPT_SPIN
uniform vec3 view_focus;uniform float view_angle;uniform float view_rand;
#endif
vec3 proj_orth(vec2 st){return vec3(st,0.0);}vec3 proj_pers_st_to_dir(vec2 st){return vec3(st,-1.0);}vec3 proj_sg_st_to_dir(vec2 st){return normalize(vec3(st+st,dot(st,st)-1.0));}vec3 proj_spherical_st_to_dir(vec2 st){vec3 dir;dir.zx=cos(st.y)*vec2(-cos(st.x),sin(st.x));dir.y=sin(st.y);return dir;}vec3 proj_cylinder_st_to_dir(vec2 st){vec3 dir;dir.zx=vec2(-cos(st.x),sin(st.x));dir.y=st.y;return dir;}float bokeh(vec2 p,float n,float ph){float angle=PI2/n;float phi=atan(p.y,p.x)+PI-ph+angle*0.5;float h=length(p)*cos(abs(mod(phi,angle)-angle*0.5));float H=cos(angle*0.5);float ratio=angle/sin(angle);return step(h,H)*ratio;}void sample_ray(vec2 st,out vec3 eye,out vec3 dir,out vec3 A){A=vec3(1.0);st*=view_fov;
#if defined(OPT_ORTH)
eye=proj_orth(st);
#else
eye=vec3(0.0);
#endif
#if defined(OPT_ORTH)
dir=vec3(0.0,0.0,-1.0);
#elif defined(OPT_PANO)
dir=proj_spherical_st_to_dir(st);
#elif defined(OPT_SG)
dir=proj_sg_st_to_dir(st);
#else
dir=proj_pers_st_to_dir(st);
#endif
#ifdef OPT_DOF
{vec2 uv=sample_disc(jitter2(dof_rand));
#ifdef OPT_BLADE
A*=vec3(bokeh(uv,dof_lens.z,dof_lens.w));
#endif
vec3 offset=vec3(uv*dof_lens.x,0.0);
#if!(defined(OPT_PERS)||defined(OPT_ORTH))
offset=n_to_rot(normalize(dir))*offset;
#endif
eye+=offset*dof_lens.y;dir-=offset;}
#endif
eye=view_rot*eye+view_origin;dir=view_rot*dir;
#ifdef OPT_SPIN
{mat3 M=rotz(view_angle*(jitter1(view_rand)-0.5));eye=M*(eye-view_focus)+view_focus;dir=M*dir;}
#endif
dir=normalize(dir);
#ifdef OPT_ORTH
CorrectDepth(eye,dir);
#endif
}

