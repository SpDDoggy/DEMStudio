// DEMStudio uniform cubic B-spline surface. Original implementation.
// Positive 4x4 basis: smooth approximation (not interpolation), without overshoot.
// The existing conservative mip texture is queried with an expanded footprint.
uniform vec2 bc_source_size;
uniform vec2 bc_source_offset;
// A leaf bound and its accepted patch use the same raw LOD0 footprint.
// Cache before height scaling/quantization so both original operation orders stay intact.
float bc_leaf_samples[16];
vec2 bc_leaf_cell,bc_leaf_lo;
bool bc_leaf_ready=false;
float bc_raw(vec2 logical,bool reuse){
    vec2 p=clamp(logical-bc_source_offset,vec2(0.),bc_source_size-vec2(1.));
    float raw;
    if(reuse){
        ivec2 index=ivec2(p-bc_leaf_lo);
        raw=bc_leaf_samples[index.y*4+index.x];
    }else raw=texture2DLod(mipmap,(p+vec2(.5))*mipmap_tex_inv_size,0.).x;
    float h=raw*mipmap_size.z;
#ifdef OPT_LEVEL
    h=ceil(h/mipmap_step)*mipmap_step;
#endif
    return h+mipmap_size.w;
}
float bc_bound(vec2 p,float lod){
    float scale=exp2(lod);
    vec2 cell=floor(p/scale)*scale;
    vec2 lo=clamp(cell-vec2(1.)-bc_source_offset,vec2(0.),bc_source_size-vec2(1.));
    vec2 hi=clamp(cell+vec2(scale+1.)-bc_source_offset,vec2(0.),bc_source_size-vec2(1.));
    lo=floor(lo/scale);hi=floor(hi/scale);
    bc_leaf_ready=lod==0.;
    if(bc_leaf_ready){bc_leaf_cell=cell;bc_leaf_lo=lo;}
    float h=0.;
    // At level 0 this is a 4x4 footprint; at higher levels at most 3x3.
    for(int y=0;y<4;y++)for(int x=0;x<4;x++){
        if(lod>=.5&&(x>=3||y>=3))continue;
        vec2 index=lo+vec2(x,y);
        if(any(greaterThan(index,hi)))continue;
        vec2 q=index*scale;
        float raw=texture2DLod(mipmap,(q+vec2(.5))*mipmap_tex_inv_size,lod).x;
        if(bc_leaf_ready)bc_leaf_samples[y*4+x]=raw;
        h=max(h,raw);
    }
    h*=mipmap_size.z;
#ifdef OPT_LEVEL
    h=ceil(h/mipmap_step)*mipmap_step;
#endif
    return h+mipmap_size.w;
}
vec4 bc_to_bezier(vec4 p){
    return vec4((p.x+4.*p.y+p.z)/6.,(2.*p.y+p.z)/3.,
                (p.y+2.*p.z)/3.,(p.y+4.*p.z+p.w)/6.);
}
void bc_patch(vec2 cell,out float p[16]){
    bool reuse=bc_leaf_ready&&all(equal(cell,bc_leaf_cell));
    for(int y=0;y<4;y++){
        vec4 row;
        for(int x=0;x<4;x++)row[x]=bc_raw(cell+vec2(x-1,y-1),reuse);
        row=bc_to_bezier(row);
        for(int x=0;x<4;x++)p[y*4+x]=row[x];
    }
    for(int x=0;x<4;x++){
        vec4 col=bc_to_bezier(vec4(p[x],p[4+x],p[8+x],p[12+x]));
        for(int y=0;y<4;y++)p[y*4+x]=col[y];
    }
}
vec4 bc_basis(float t){float s=1.-t;return vec4(s*s*s,3.*s*s*t,3.*s*t*t,t*t*t);}
vec4 bc_derivative(float t){float s=1.-t;return vec4(-3.*s*s,3.*s*s-6.*s*t,6.*s*t-3.*t*t,3.*t*t);}
vec3 bc_value_gradient(float p[16],vec2 uv){
    vec4 u=bc_basis(uv.x),v=bc_basis(uv.y),du=bc_derivative(uv.x),dv=bc_derivative(uv.y);
    vec4 rows,dx;float reference=p[5];
    for(int j=0;j<4;j++){
        vec4 row=vec4(p[j*4],p[j*4+1],p[j*4+2],p[j*4+3])-vec4(reference);
        rows[j]=dot(row,u);dx[j]=dot(row,du);
    }
    return vec3(reference+dot(rows,v),dot(dx,v),dot(rows,dv));
}
float bc_blossom(vec4 c,vec3 t){vec3 a=mix(c.xyz,c.yzw,t.x);vec2 b=mix(a.xy,a.yz,t.y);return mix(b.x,b.y,t.z);}
vec4 bc_restrict(vec4 c,float a,float b){
    return vec4(bc_blossom(c,vec3(a)),bc_blossom(c,vec3(a,a,b)),
                bc_blossom(c,vec3(a,b,b)),bc_blossom(c,vec3(b)));
}
float bc_binomial6(int i){if(i==0||i==6)return 1.;if(i==1||i==5)return 6.;if(i==2||i==4)return 15.;return 20.;}
void bc_ray_polynomial(float p[16],vec2 a,vec2 b,float z0,float z1,out float c[7]){
    float restricted[16];
    for(int y=0;y<4;y++){
        vec4 row=bc_restrict(vec4(p[y*4],p[y*4+1],p[y*4+2],p[y*4+3]),a.x,b.x);
        for(int x=0;x<4;x++)restricted[y*4+x]=row[x];
    }
    for(int x=0;x<4;x++){
        vec4 col=bc_restrict(vec4(restricted[x],restricted[x+4],restricted[x+8],restricted[x+12]),a.y,b.y);
        for(int y=0;y<4;y++)restricted[y*4+x]=col[y];
    }
    vec4 choose3=vec4(1.,3.,3.,1.);
    for(int k=0;k<7;k++){
        float value=0.;
        for(int x=0;x<4;x++){int y=k-x;if(y>=0&&y<4)value+=restricted[y*4+x]*choose3[x]*choose3[y]/bc_binomial6(k);}
        c[k]=value-mix(z0,z1,float(k)/6.);
    }
}
// Ordered Bernstein convex-hull isolation of the earliest zero / solid entry.
// No fixed ray marching: every rejected interval has a conservative sign bound.
// Depth 24 bounds normalized-t interval width by 2^-24. Ambiguous intervals are kept
// as contact only at parameter precision exhaustion; negative hulls are clear.
bool bc_first_root_isolated(float coefficients[7],out float root){
    float stack[175],starts[25],ends[25];int depths[25];
    for(int k=0;k<7;k++)stack[k]=coefficients[k];
    starts[0]=0.;ends[0]=1.;depths[0]=0;int sp=0;
    while(sp>=0){
        float c[7];for(int k=0;k<7;k++)c[k]=stack[sp*7+k];
        float lo=starts[sp],hi=ends[sp];int depth=depths[sp];sp--;
        float low=c[0],high=c[0];
        for(int k=1;k<7;k++){low=min(low,c[k]);high=max(high,c[k]);}
        if(high<0.)continue;
        if(c[0]>0. || (low==0.&&high==0.)) {root=lo;return true;}
        float mid=(lo+hi)*.5;
        if(depth>=24||mid<=lo||mid>=hi){root=lo;return true;}
        float work[7],left[7],right[7];
        for(int k=0;k<7;k++)work[k]=c[k];left[0]=work[0];right[6]=work[6];
        for(int d=1;d<7;d++){
            for(int k=0;k<7-d;k++)work[k]=(work[k]+work[k+1])*.5;
            left[d]=work[0];right[6-d]=work[6-d];
        }
        // Push right then left: DFS processes increasing ray distance.
        sp++;starts[sp]=mid;ends[sp]=hi;depths[sp]=depth+1;
        for(int k=0;k<7;k++)stack[sp*7+k]=right[k];
        sp++;starts[sp]=lo;ends[sp]=mid;depths[sp]=depth+1;
        for(int k=0;k<7;k++)stack[sp*7+k]=left[k];
    }
    return false;
}
// Ordered control values certify a nondecreasing polynomial: its derivative is
// a positive Bernstein combination of their adjacent differences. Opposite,
// strict endpoint signs then certify exactly one solid entry. Restrict the same
// control net as the general isolator, retaining only the half containing it;
// identical midpoint arithmetic preserves its 24-bit parameter bound without
// a root stack or traversal of rejected left siblings. Zero endpoints and any
// nonmonotone curve keep the general contact / earliest-root semantics.
bool bc_first_root(float coefficients[7],out float root){
    bool increasing=coefficients[0]<0.&&coefficients[6]>0.;
    for(int k=0;k<6;k++)increasing=increasing&&(coefficients[k+1]>=coefficients[k]);
    if(!increasing)return bc_first_root_isolated(coefficients,root);
    float c[7];for(int k=0;k<7;k++)c[k]=coefficients[k];
    float lo=0.,hi=1.;
    for(int depth=0;depth<24;depth++){
        if(c[0]==0.&&c[6]==0.)break;
        float mid=(lo+hi)*.5;
        if(mid<=lo||mid>=hi)break;
        float work[7],left[7],right[7];
        for(int k=0;k<7;k++)work[k]=c[k];left[0]=work[0];right[6]=work[6];
        for(int d=1;d<7;d++){
            for(int k=0;k<7-d;k++)work[k]=(work[k]+work[k+1])*.5;
            left[d]=work[0];right[6-d]=work[6-d];
        }
        if(work[0]>=0.){
            hi=mid;for(int k=0;k<7;k++)c[k]=left[k];
        }else{
            lo=mid;for(int k=0;k<7;k++)c[k]=right[k];
        }
    }
    root=lo;return true;
}
bool bc_intersection(vec3 o,vec3 dir,vec2 interval,vec2 cell,out float distance,out vec3 normal){
    // Apply the kernel MIN distance to every ray to suppress immediate self-contact;
    // primary and secondary rays then use exactly the same intersection geometry.
    interval.x=max(interval.x,MIN);
    if(interval.x>=interval.y)return false;
    float p[16];bc_patch(cell,p);
    vec3 a=o+dir*interval.x,b=o+dir*interval.y;
    if(dir.x==0.&&dir.y==0.){
        vec3 value=bc_value_gradient(p,clamp(a.xy-cell,vec2(0.),vec2(1.)));
        float t=(value.x-o.z)/dir.z;
        if(dir.z*interval.x<value.x-o.z)t=interval.x;
        if(t<interval.x||t>interval.y)return false;
        distance=t;normal=normalize(vec3(-value.y,-value.z,1.));return true;
    }
    vec2 ua=clamp(a.xy-cell,vec2(0.),vec2(1.)),ub=clamp(b.xy-cell,vec2(0.),vec2(1.));
    // Work in ray-origin-relative height: o.z + dz*MIN can round back to o.z
    // at large altitude. Subtracting o.z first preserves the outgoing gap.
    float local_patch[16];for(int k=0;k<16;k++)local_patch[k]=p[k]-o.z;
    float c[7];bc_ray_polynomial(local_patch,ua,ub,dir.z*interval.x,dir.z*interval.y,c);
    float t;if(!bc_first_root(c,t))return false;
    distance=mix(interval.x,interval.y,t);
    vec2 uv=clamp((o+dir*distance).xy-cell,vec2(0.),vec2(1.));
    vec3 h=bc_value_gradient(p,uv);normal=normalize(vec3(-h.y,-h.z,1.));
    return true;
}
bool ray_vs_prim(vec3 o,vec3 dir,inout vec2 e,inout vec3 n,vec2 cell,vec4 unused,float scale,inout vec3 target){
    float d;vec3 normal;if(!bc_intersection(o,dir,e,cell,d,normal))return false;
    e.x=d;n=normal;target=o+dir*d;return true;
}
float ray_vs_prim_shadow(vec3 o,vec3 dir,vec2 e,vec2 cell,vec4 unused,float scale){
    e.x=max(e.x,MIN);
    if(e.x>=e.y)return 1.;
    float p[16];bc_patch(cell,p);
    vec3 a=o+dir*e.x,b=o+dir*e.y;
    if(dir.x==0.&&dir.y==0.){
        float h=bc_value_gradient(p,clamp(a.xy-cell,vec2(0.),vec2(1.))).x-o.z;
        float t=h/dir.z;
        if(dir.z*e.x<h)t=e.x;
        return t>=e.x&&t<=e.y?0.:1.;
    }
    vec2 ua=clamp(a.xy-cell,vec2(0.),vec2(1.)),ub=clamp(b.xy-cell,vec2(0.),vec2(1.));
    float local_patch[16];for(int k=0;k<16;k++)local_patch[k]=p[k]-o.z;
    float c[7];bc_ray_polynomial(local_patch,ua,ub,dir.z*e.x,dir.z*e.y,c);
    // Visibility needs existence, not the earliest contact or a shading normal.
    // A strictly solid endpoint certifies occlusion by continuity. Exact-zero
    // endpoints retain the isolator's existing tangent / outgoing semantics.
    if(c[0]>0.||c[6]>0.)return 0.;
    float high=c[0];for(int k=1;k<7;k++)high=max(high,c[k]);
    if(high<0.)return 1.;
    float root;return bc_first_root(c,root)?0.:1.;
}
