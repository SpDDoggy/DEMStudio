// DEMStudio uniform cubic B-spline surface. Original implementation.
// Positive 4x4 basis: smooth approximation (not interpolation), without overshoot.
// The existing conservative mip texture is queried with an expanded footprint.
uniform vec2 bc_source_size;
uniform vec2 bc_source_offset;
#ifdef BC_PROFILE_COUNTERS
#define BC_COUNT(i) bc_profile_local[i]++
#define BC_DEPTH(d) bc_profile_local[7]=max(bc_profile_local[7],uint(d))
#else
#define BC_COUNT(i)
#define BC_DEPTH(d)
#endif
// A leaf bound and its accepted patch use the same raw LOD0 footprint.
// Cache before height scaling/quantization so both original operation orders stay intact.
#ifndef BC_LEAF_BOUND_TEX
float bc_leaf_samples[16];
vec2 bc_leaf_cell,bc_leaf_lo;
#endif
bool bc_leaf_ready=false;
float bc_raw(vec2 logical,bool reuse){
    vec2 p=clamp(logical-bc_source_offset,vec2(0.),bc_source_size-vec2(1.));
    float raw;
#ifndef BC_LEAF_BOUND_TEX
    if(reuse){
        ivec2 index=ivec2(p-bc_leaf_lo);
        raw=bc_leaf_samples[index.y*4+index.x];
    }else raw=texture2DLod(mipmap,(p+vec2(.5))*mipmap_tex_inv_size,0.).x;
#else
    raw=texture2DLod(mipmap,(p+vec2(.5))*mipmap_tex_inv_size,0.).x;
#endif
    float h=raw*mipmap_size.z;
#ifdef OPT_LEVEL
    h=ceil(h/mipmap_step)*mipmap_step;
#endif
    return h+mipmap_size.w;
}
#ifdef BC_PATCH_GATHER
float bc_gather_height(float raw){
    float h=raw*mipmap_size.z;
#ifdef OPT_LEVEL
    h=ceil(h/mipmap_step)*mipmap_step;
#endif
    return h+mipmap_size.w;
}
#endif

float bc_bound(vec2 p,float lod){
#ifdef BC_PROFILE_COUNTERS
    if(lod==0.){BC_COUNT(1);}else{BC_COUNT(0);}
#endif
    float scale=exp2(lod);
    vec2 cell=floor(p/scale)*scale;
#ifdef BC_LEAF_BOUND_TEX
    if(lod==0.){
        float h=pg_leaf_raw(cell);
        h*=mipmap_size.z;
#ifdef OPT_LEVEL
        h=ceil(h/mipmap_step)*mipmap_step;
#endif
        return h+mipmap_size.w;
    }
#endif
#ifdef BC_PRECOMPUTED_BOUNDS
    if(lod>=.5&&pg_precomputed_bounds!=0){
        bc_leaf_ready=false;
        float h=pg_bound_raw(cell,lod);
        h*=mipmap_size.z;
#ifdef OPT_LEVEL
        h=ceil(h/mipmap_step)*mipmap_step;
#endif
        return h+mipmap_size.w;
    }
#endif
    vec2 lo=clamp(cell-vec2(1.)-bc_source_offset,vec2(0.),bc_source_size-vec2(1.));
    vec2 hi=clamp(cell+vec2(scale+1.)-bc_source_offset,vec2(0.),bc_source_size-vec2(1.));
    lo=floor(lo/scale);hi=floor(hi/scale);
#ifndef BC_LEAF_BOUND_TEX
    bc_leaf_ready=lod==0.;
    if(bc_leaf_ready){bc_leaf_cell=cell;bc_leaf_lo=lo;}
#endif
    float h=0.;
    // At level 0 this is a 4x4 footprint; at higher levels at most 3x3.
    for(int y=0;y<4;y++)for(int x=0;x<4;x++){
        if(lod>=.5&&(x>=3||y>=3))continue;
        vec2 index=lo+vec2(x,y);
        if(any(greaterThan(index,hi)))continue;
        vec2 q=index*scale;
        float raw=texture2DLod(mipmap,(q+vec2(.5))*mipmap_tex_inv_size,lod).x;
#ifndef BC_LEAF_BOUND_TEX
        if(bc_leaf_ready)bc_leaf_samples[y*4+x]=raw;
#endif
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
#if defined(BC_ROW_CACHE) || defined(BC_ROW_BOUND)
uniform sampler2DArray pg_bicubic_rows;
#endif
#ifdef BC_ROW_BOUND
bool bc_row_clear(vec2 cell,vec3 o,vec3 dir,vec2 e){
#ifdef OPT_LEVEL
    return false;
#else
    if(!all(lessThanEqual(abs(cell),vec2(1e8)))||any(notEqual(cell,floor(cell)))
        ||!all(lessThanEqual(abs(o),vec3(1e25)))||!all(lessThanEqual(abs(dir),vec3(1e25)))
        ||!all(lessThanEqual(abs(e),vec2(1e25)))||(dir.x==0.&&dir.y==0.)
        ||!all(greaterThan(bc_source_size,vec2(0.)))||!all(lessThanEqual(bc_source_size,vec2(1e8)))
        ||!all(lessThanEqual(abs(bc_source_offset),vec2(1.))))return false;
    float z0=dir.z*max(e.x,MIN),z1=dir.z*e.y;
    float high=-1e30,magnitude=0.;bool valid=true;
#ifdef BC_CELL_BOUND
    ivec2 index=ivec2(cell)-pg_brick_core;
    if(any(lessThan(index,ivec2(0)))||any(greaterThanEqual(index,ivec2(514,516))))return false;
    vec4 bound=texelFetch(pg_bicubic_rows,ivec3(index,pg_brick_layer),0);
    valid=bound.z==1.&&abs(bound.x)<=1e25&&bound.y>=0.&&bound.y<=1e25;
    high=bound.x;magnitude=bound.y;
#else
    for(int j=0;j<4;j++){
        float ry=clamp(cell.y+float(j-1)-bc_source_offset.y,0.,bc_source_size.y-1.);
        ivec2 index=ivec2(cell.x-float(pg_brick_core.x),ry-float(pg_brick_core.y)+1.);
        if(any(lessThan(index,ivec2(0)))||any(greaterThanEqual(index,ivec2(514,516))))return false;
        vec4 row=texelFetch(pg_bicubic_rows,ivec3(index,pg_brick_layer),0);
        valid=valid&&row.z==1.&&abs(row.x)<=1e25&&row.y>=0.&&row.y<=1e25;
        high=max(high,row.x);magnitude=max(magnitude,row.y);
    }
#endif
    float scale=abs(mipmap_size.z)+abs(mipmap_size.w)+abs(o.z)+magnitude+max(abs(z0),abs(z1));
    return valid&&abs(z0)<=1e25&&abs(z1)<=1e25&&scale<=1e25
        &&min(z0,z1)>high-o.z+max(1e-5,scale*1e-3);
#endif
}
#endif
#ifdef BC_PATCH_GATHER
bool bc_patch_gather_raw(vec2 cell,out vec4 rows[4]){
    if(any(notEqual(cell,floor(cell))))return false;
    vec2 lo=cell-vec2(1.)-bc_source_offset;
    vec2 hi=lo+vec2(3.);
    if(any(lessThan(lo,vec2(0.)))||any(greaterThanEqual(hi,bc_source_size)))return false;
    vec2 local=lo-vec2(pg_brick_core)+vec2(64.);
    if(any(lessThan(local,vec2(0.)))||any(greaterThan(local,vec2(636.))))return false;
    vec4 q00=textureGather(pg_low_height,vec3((local+vec2(1.))/640.,float(pg_brick_layer)),0).wzxy;
    vec4 q20=textureGather(pg_low_height,vec3((local+vec2(3.,1.))/640.,float(pg_brick_layer)),0).wzxy;
    vec4 q02=textureGather(pg_low_height,vec3((local+vec2(1.,3.))/640.,float(pg_brick_layer)),0).wzxy;
    vec4 q22=textureGather(pg_low_height,vec3((local+vec2(3.))/640.,float(pg_brick_layer)),0).wzxy;
    rows[0]=vec4(q00.xy,q20.xy);rows[1]=vec4(q00.zw,q20.zw);
    rows[2]=vec4(q02.xy,q22.xy);rows[3]=vec4(q02.zw,q22.zw);
    return true;
}
#endif
void bc_patch(vec2 cell,out float p[16]){
    BC_COUNT(2);
#ifdef BC_PATCH_GATHER
    vec4 bc_patch_gather_rows[4];
    bool bc_patch_gather_ready=bc_patch_gather_raw(cell,bc_patch_gather_rows);
#endif
#ifdef BC_LEAF_BOUND_TEX
    bool reuse=false;
#else
    bool reuse=bc_leaf_ready&&all(equal(cell,bc_leaf_cell));
#endif
    for(int y=0;y<4;y++){
        vec4 row;
#ifdef BC_PATCH_GATHER
        if(bc_patch_gather_ready){
            for(int x=0;x<4;x++)row[x]=bc_gather_height(bc_patch_gather_rows[y][x]);
        }else{
            for(int x=0;x<4;x++)row[x]=bc_raw(cell+vec2(x-1,y-1),reuse);
        }
        row=bc_to_bezier(row);
#elif defined(BC_ROW_CACHE)
        float rawY=clamp(cell.y+float(y-1)-bc_source_offset.y,0.,bc_source_size.y-1.);
        ivec2 rowIndex=ivec2(cell.x-float(pg_brick_core.x),rawY-float(pg_brick_core.y)+1.);
        if(all(greaterThanEqual(rowIndex,ivec2(0)))&&all(lessThan(rowIndex,ivec2(514,516)))){
            row=texelFetch(pg_bicubic_rows,ivec3(rowIndex,pg_brick_layer),0);
        }else{
            for(int x=0;x<4;x++)row[x]=bc_raw(cell+vec2(x-1,y-1),reuse);
            row=bc_to_bezier(row);
        }
#else
        for(int x=0;x<4;x++)row[x]=bc_raw(cell+vec2(x-1,y-1),reuse);
        row=bc_to_bezier(row);
#endif
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
    BC_COUNT(11);
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
#ifdef BC_ROOT_REBUILD_KEYS
void bc_rebuild_interval(uint key,out float lo,out float hi,out int depth){
    depth=0;uint cursor=key;while(cursor>1u){cursor=cursor>>1u;depth++;}
    uint index=key-(1u<<uint(depth));
    float scale=uintBitsToFloat(uint(127-depth)<<23u);
    lo=float(index)*scale;hi=float(index+1u)*scale;
}
#endif
#ifdef BC_ROOT_DYADIC_KEYS
void bc_key_interval(uint key,out float lo,out float hi,out int depth){
    depth=findMSB(key);
    uint index=key-(1u<<uint(depth));
    // Exact IEEE power of two; avoid relying on exp2 approximation accuracy.
    float scale=uintBitsToFloat(uint(127-depth)<<23u);
    lo=float(index)*scale;hi=float(index+1u)*scale;
}
#endif
#ifndef BC_REBUILD_SPLIT
#define BC_REBUILD_SPLIT()
#define BC_REBUILD_POP()
#define BC_REBUILD_REPLAY()
#endif
#ifndef BC_REBUILD_CACHE_HIT
#define BC_REBUILD_CACHE_HIT()
#endif
#ifdef BC_ROOT_V2
#ifndef BC_ROOT_V2_HIT
#define BC_ROOT_V2_HIT()
#endif
// A child certificate only changes DFS control. Keep global dyadic endpoints
// and the original split expressions; overflow-risk inputs retain the isolator.
bool bc_root_v2_child(float coefficients[7],float start,float end,int initial_depth,out float root){
    bool certified=coefficients[0]<0.&&coefficients[6]>0.;
#ifndef BC_ROOT_V2_ENDPOINT_CERT
    for(int k=0;k<7;k++)certified=certified&&!isnan(coefficients[k])&&!isinf(coefficients[k])&&abs(coefficients[k])<=1e30;
#endif
    for(int k=0;k<6;k++)certified=certified&&coefficients[k+1]>=coefficients[k];
#ifdef BC_ROOT_V2_ENDPOINT_CERT
    // Ordered adjacent comparisons exclude NaNs and enclose every coefficient
    // between the endpoints. These bounds therefore imply the original seven
    // finite/absolute-value checks, including infinities and signed zero.
    certified=certified&&coefficients[0]>=-1e30&&coefficients[6]<=1e30;
#endif
    if(!certified)return false;
    BC_ROOT_V2_HIT();
#ifdef BC_PROFILE_COUNTERS
    bc_profile_root_note(8);
#endif
    float c[7];for(int k=0;k<7;k++)c[k]=coefficients[k];
    float lo=start,hi=end;
    for(int depth=initial_depth;depth<24;depth++){
        if(c[0]==0.&&c[6]==0.)break;
        float mid=(lo+hi)*.5;
        if(mid<=lo||mid>=hi)break;
        BC_COUNT(15);BC_REBUILD_SPLIT();
#ifdef BC_ROOT_V2_INPLACE_LEFT
        float work[7],right[7];
        for(int k=0;k<7;k++)work[k]=c[k];right[6]=work[6];
        for(int d=1;d<7;d++){
            for(int k=0;k<7-d;k++)work[k]=(work[k]+work[k+1])*.5;
            c[d]=work[0];right[6-d]=work[6-d];
        }
        if(work[0]>=0.){hi=mid;}
        else{lo=mid;for(int k=0;k<7;k++)c[k]=right[k];}
#else
        float work[7],left[7],right[7];
        for(int k=0;k<7;k++)work[k]=c[k];left[0]=work[0];right[6]=work[6];
        for(int d=1;d<7;d++){
            for(int k=0;k<7-d;k++)work[k]=(work[k]+work[k+1])*.5;
            left[d]=work[0];right[6-d]=work[6-d];
        }
        if(work[0]>=0.){hi=mid;for(int k=0;k<7;k++)c[k]=left[k];}
        else{lo=mid;for(int k=0;k<7;k++)c[k]=right[k];}
#endif
    }
    root=lo;return true;
}
#endif
bool bc_first_root_isolated(float coefficients[7],out float root){
#ifdef BC_PROFILE_COUNTERS
    bc_profile_root_note(2);
#endif
#ifdef BC_ROOT_REBUILD_KEYS
#ifdef BC_ROOT_RIGHT_CACHE
    uint cached_key=0u;float cached_right[7];
#endif
#ifdef BC_ROOT_STACKLESS_KEYS
    uint key=1u;
#else
    uint keys[25];int sp=-1;uint key=1u;
#endif
    float c[7];for(int k=0;k<7;k++)c[k]=coefficients[k];
    float lo=0.,hi=1.;int depth=0;bool take_pending=false;
    while(true){
        if(take_pending){
#ifdef BC_ROOT_STACKLESS_KEYS
            // Left-first DFS successor: climb completed right edges, then
            // enter the nearest pending right sibling. The path key already
            // encodes every pending sibling; no indexed local-memory stack.
            while((key&1u)!=0u)key=key>>1u;
            if(key==0u)break;
            key=key|1u;BC_REBUILD_POP();
#else
            if(sp<0)break;
            key=keys[sp];sp--;BC_REBUILD_POP();
#endif
            bc_rebuild_interval(key,lo,hi,depth);
#ifdef BC_ROOT_RIGHT_CACHE
            if(key==cached_key){
                for(int k=0;k<7;k++)c[k]=cached_right[k];
                cached_key=0u;BC_REBUILD_CACHE_HIT();
            }else{
#endif
            for(int k=0;k<7;k++)c[k]=coefficients[k];
            for(int level=depth-1;level>=0;level--){
                BC_REBUILD_REPLAY();
#ifdef BC_PROFILE_COUNTERS
                bc_profile_replay_splits++;
#endif
                float work[7],left[7],right[7];
                for(int k=0;k<7;k++)work[k]=c[k];left[0]=work[0];right[6]=work[6];
                for(int d=1;d<7;d++){
                    for(int k=0;k<7-d;k++)work[k]=(work[k]+work[k+1])*.5;
                    left[d]=work[0];right[6-d]=work[6-d];
                }
                if(((key>>uint(level))&1u)==0u){for(int k=0;k<7;k++)c[k]=left[k];}
                else{for(int k=0;k<7;k++)c[k]=right[k];}
            }
#ifdef BC_ROOT_RIGHT_CACHE
            }
#endif
        }
        BC_COUNT(5);BC_DEPTH(depth);
        float low=c[0],high=c[0];
        for(int k=1;k<7;k++){low=min(low,c[k]);high=max(high,c[k]);}
        if(high<0.){BC_COUNT(6);
#ifdef BC_PROFILE_COUNTERS
            bc_profile_top_negative(depth);
#endif
            take_pending=true;continue;}
        if(c[0]>0. || (low==0.&&high==0.)) {root=lo;return true;}
        float mid=(lo+hi)*.5;
        if(depth>=24||mid<=lo||mid>=hi){root=lo;return true;}
#ifdef BC_ROOT_V2
        if(bc_root_v2_child(c,lo,hi,depth,root))return true;
#endif
        BC_COUNT(15);BC_REBUILD_SPLIT();
        float work[7],left[7],right[7];
        for(int k=0;k<7;k++)work[k]=c[k];left[0]=work[0];right[6]=work[6];
        for(int d=1;d<7;d++){
            for(int k=0;k<7-d;k++)work[k]=(work[k]+work[k+1])*.5;
            left[d]=work[0];right[6-d]=work[6-d];
        }
#ifndef BC_ROOT_STACKLESS_KEYS
        sp++;keys[sp]=(key<<1u)|1u;
#endif
#ifdef BC_ROOT_RIGHT_CACHE
        cached_key=(key<<1u)|1u;
        for(int k=0;k<7;k++)cached_right[k]=right[k];
#endif
        key=key<<1u;hi=mid;depth=depth+1;
        for(int k=0;k<7;k++)c[k]=left[k];
        take_pending=false;
    }
#else
#ifdef BC_ROOT_STACK_LEFT_CONTINUE
    float stack[175],starts[25],ends[25];int depths[25];
    float c[7];for(int k=0;k<7;k++)c[k]=coefficients[k];
    float lo=0.,hi=1.;int depth=0,sp=-1;
    bool take_pending=false;
    while(true){
        if(take_pending){
            if(sp<0)break;
            for(int k=0;k<7;k++)c[k]=stack[sp*7+k];
            lo=starts[sp];hi=ends[sp];depth=depths[sp];sp--;
        }
        BC_COUNT(5);BC_DEPTH(depth);
        float low=c[0],high=c[0];
        for(int k=1;k<7;k++){low=min(low,c[k]);high=max(high,c[k]);}
        if(high<0.){BC_COUNT(6);
#ifdef BC_PROFILE_COUNTERS
            bc_profile_top_negative(depth);
#endif
            take_pending=true;continue;}
        if(c[0]>0. || (low==0.&&high==0.)) {root=lo;return true;}
        float mid=(lo+hi)*.5;
        if(depth>=24||mid<=lo||mid>=hi){root=lo;return true;}
#ifdef BC_ROOT_V2
        if(bc_root_v2_child(c,lo,hi,depth,root))return true;
#endif
        BC_COUNT(15);
        float work[7],left[7],right[7];
        for(int k=0;k<7;k++)work[k]=c[k];left[0]=work[0];right[6]=work[6];
        for(int d=1;d<7;d++){
            for(int k=0;k<7-d;k++)work[k]=(work[k]+work[k+1])*.5;
            left[d]=work[0];right[6-d]=work[6-d];
        }
        // Only the right sibling waits in the stack. Process the same left
        // coefficients next, preserving midpoint arithmetic and DFS order.
        sp++;starts[sp]=mid;ends[sp]=hi;depths[sp]=depth+1;
        for(int k=0;k<7;k++)stack[sp*7+k]=right[k];
        hi=mid;depth=depth+1;
        for(int k=0;k<7;k++)c[k]=left[k];
        take_pending=false;
    }
#else
    float stack[175];
#ifdef BC_ROOT_DYADIC_KEYS
    uint keys[25];keys[0]=1u;
#else
    float starts[25],ends[25];int depths[25];
    starts[0]=0.;ends[0]=1.;depths[0]=0;
#endif
    for(int k=0;k<7;k++)stack[k]=coefficients[k];
    int sp=0;
    while(sp>=0){
        float c[7];for(int k=0;k<7;k++)c[k]=stack[sp*7+k];
#ifdef BC_ROOT_DYADIC_KEYS
        uint key=keys[sp];float lo,hi;int depth;bc_key_interval(key,lo,hi,depth);sp--;
#else
        float lo=starts[sp],hi=ends[sp];int depth=depths[sp];sp--;
#endif
        BC_COUNT(5);BC_DEPTH(depth);
        float low=c[0],high=c[0];
        for(int k=1;k<7;k++){low=min(low,c[k]);high=max(high,c[k]);}
        if(high<0.){BC_COUNT(6);
#ifdef BC_PROFILE_COUNTERS
            bc_profile_top_negative(depth);
#endif
            continue;}
        if(c[0]>0. || (low==0.&&high==0.)) {root=lo;return true;}
        float mid=(lo+hi)*.5;
        if(depth>=24||mid<=lo||mid>=hi){root=lo;return true;}
#ifdef BC_ROOT_V2
        if(bc_root_v2_child(c,lo,hi,depth,root))return true;
#endif
        BC_COUNT(15);
        float work[7],left[7],right[7];
        for(int k=0;k<7;k++)work[k]=c[k];left[0]=work[0];right[6]=work[6];
        for(int d=1;d<7;d++){
            for(int k=0;k<7-d;k++)work[k]=(work[k]+work[k+1])*.5;
            left[d]=work[0];right[6-d]=work[6-d];
        }
        // Push right then left: DFS processes increasing ray distance.
        sp++;
#ifdef BC_ROOT_DYADIC_KEYS
        keys[sp]=(key<<1u)|1u;
#else
        starts[sp]=mid;ends[sp]=hi;depths[sp]=depth+1;
#endif
        for(int k=0;k<7;k++)stack[sp*7+k]=right[k];
        sp++;
#ifdef BC_ROOT_DYADIC_KEYS
        keys[sp]=key<<1u;
#else
        starts[sp]=lo;ends[sp]=mid;depths[sp]=depth+1;
#endif
        for(int k=0;k<7;k++)stack[sp*7+k]=left[k];
    }
#endif
#endif
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
    BC_COUNT(3);
#ifdef BC_ROOT_NEGATIVE_PRECHECK
    // Match the isolator's first-node rejection, including its max order and
    // strict comparison. Do not replace with an all-less-than predicate: GLSL
    // max and comparisons need not handle NaNs identically.
    float high=coefficients[0];
    for(int k=1;k<7;k++)high=max(high,coefficients[k]);
    if(high<0.){BC_COUNT(6);
#ifdef BC_PROFILE_COUNTERS
        bc_profile_root_note(4);
#endif
        return false;}
#endif
    bool increasing=coefficients[0]<0.&&coefficients[6]>0.;
    for(int k=0;k<6;k++)increasing=increasing&&(coefficients[k+1]>=coefficients[k]);
    if(!increasing)return bc_first_root_isolated(coefficients,root);
    BC_COUNT(4);
#ifdef BC_PROFILE_COUNTERS
    bc_profile_root_note(3);
#endif
    float c[7];for(int k=0;k<7;k++)c[k]=coefficients[k];
    float lo=0.,hi=1.;
    for(int depth=0;depth<24;depth++){
        if(c[0]==0.&&c[6]==0.)break;
        float mid=(lo+hi)*.5;
        if(mid<=lo||mid>=hi)break;
#ifdef BC_PROFILE_COUNTERS
        bc_profile_top_splits++;
#endif
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
#if defined(BC_ANY_ROOT_EARLY) && !defined(BC_ROOT_STACK_LEFT_CONTINUE)
// Visibility-only existential variant of the original right-then-left stack.
// The midpoint is exactly right[0] in the original traversal. A positive
// midpoint therefore certifies that the original bool result is true.
bool bc_any_root(float coefficients[7]){
    BC_COUNT(3);
#ifdef BC_PROFILE_COUNTERS
    bc_profile_root_note(2);
#endif
    float stack[175];
#ifdef BC_ROOT_DYADIC_KEYS
    uint keys[25];keys[0]=1u;
#else
    float starts[25],ends[25];int depths[25];
    starts[0]=0.;ends[0]=1.;depths[0]=0;
#endif
    for(int k=0;k<7;k++)stack[k]=coefficients[k];
    int sp=0;
    while(sp>=0){
        float c[7];for(int k=0;k<7;k++)c[k]=stack[sp*7+k];
#ifdef BC_ROOT_DYADIC_KEYS
        uint key=keys[sp];float lo,hi;int depth;bc_key_interval(key,lo,hi,depth);sp--;
#else
        float lo=starts[sp],hi=ends[sp];int depth=depths[sp];sp--;
#endif
        BC_COUNT(5);BC_DEPTH(depth);
        float low=c[0],high=c[0];
        for(int k=1;k<7;k++){low=min(low,c[k]);high=max(high,c[k]);}
        if(high<0.){BC_COUNT(6);
#ifdef BC_PROFILE_COUNTERS
            bc_profile_top_negative(depth);
#endif
            continue;}
        if(c[0]>0. || (low==0.&&high==0.))return true;
        float mid=(lo+hi)*.5;
        if(depth>=24||mid<=lo||mid>=hi)return true;
        BC_COUNT(15);
        float work[7],left[7],right[7];
        for(int k=0;k<7;k++)work[k]=c[k];left[0]=work[0];right[6]=work[6];
        for(int d=1;d<7;d++){
            for(int k=0;k<7-d;k++)work[k]=(work[k]+work[k+1])*.5;
            left[d]=work[0];right[6-d]=work[6-d];
        }
        if(work[0]>0.)return true;
        sp++;
#ifdef BC_ROOT_DYADIC_KEYS
        keys[sp]=(key<<1u)|1u;
#else
        starts[sp]=mid;ends[sp]=hi;depths[sp]=depth+1;
#endif
        for(int k=0;k<7;k++)stack[sp*7+k]=right[k];
        sp++;
#ifdef BC_ROOT_DYADIC_KEYS
        keys[sp]=key<<1u;
#else
        starts[sp]=lo;ends[sp]=mid;depths[sp]=depth+1;
#endif
        for(int k=0;k<7;k++)stack[sp*7+k]=left[k];
    }
    return false;
}
#endif
#ifdef BC_PROFILE_COUNTERS
// Wrap complete root calls rather than infer task difficulty from child-node
// rejection totals. These wrappers and all additional counters disappear when
// BicubicProfile is off; the original solver bodies retain their arithmetic.
bool bc_profile_first_root(float coefficients[7],out float root,bool shadow){
    uint before=bc_profile_root_begin(shadow);
    bool hit=bc_first_root(coefficients,root);
    bc_profile_root_end(before);
    return hit;
}
#if defined(BC_ANY_ROOT_EARLY) && !defined(BC_ROOT_STACK_LEFT_CONTINUE)
bool bc_profile_any_root(float coefficients[7]){
    uint before=bc_profile_root_begin(true);
    bool hit=bc_any_root(coefficients);
    bc_profile_root_end(before);
    return hit;
}
#endif
#define BC_FIRST_ROOT(c,t,shadow) bc_profile_first_root(c,t,shadow)
#define BC_ANY_ROOT(c) bc_profile_any_root(c)
#else
#define BC_FIRST_ROOT(c,t,shadow) bc_first_root(c,t)
#define BC_ANY_ROOT(c) bc_any_root(c)
#endif
#ifdef BC_PATCH_REJECT
bool bc_patch_clear(float p[16],vec2 a,vec2 b,float z0,float z1){
    float high=p[0],magnitude=0.;
    bool valid=all(greaterThanEqual(a,vec2(0.)))&&all(lessThanEqual(a,vec2(1.)))
        &&all(greaterThanEqual(b,vec2(0.)))&&all(lessThanEqual(b,vec2(1.)))
        &&abs(z0)<=1e25&&abs(z1)<=1e25;
    for(int k=0;k<16;k++){
        valid=valid&&abs(p[k])<=1e25;
        high=max(high,p[k]);magnitude=max(magnitude,abs(p[k]));
    }
    float zmag=max(abs(z0),abs(z1));
    float margin=max(1e-6,(magnitude+zmag)*1e-4);
    return valid&&magnitude<=1e25&&zmag<=1e25&&min(z0,z1)>high+margin;
}
#endif
bool bc_intersection(vec3 o,vec3 dir,vec2 interval,vec2 cell,out float distance,out vec3 normal){
    BC_COUNT(8);
    // Apply the kernel MIN distance to every ray to suppress immediate self-contact;
    // primary and secondary rays then use exactly the same intersection geometry.
    interval.x=max(interval.x,MIN);
    if(interval.x>=interval.y)return false;
#ifdef BC_ROW_BOUND
    if(bc_row_clear(cell,o,dir,interval))return false;
#endif
    float p[16];bc_patch(cell,p);
    vec3 a=o+dir*interval.x,b=o+dir*interval.y;
    if(dir.x==0.&&dir.y==0.){
        BC_COUNT(10);
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
#ifdef BC_PATCH_REJECT
    if(bc_patch_clear(local_patch,ua,ub,dir.z*interval.x,dir.z*interval.y))return false;
#endif
    float c[7];bc_ray_polynomial(local_patch,ua,ub,dir.z*interval.x,dir.z*interval.y,c);
#ifdef BC_PROFILE_COUNTERS
    bc_profile_task(false);
#endif
    float t;if(!BC_FIRST_ROOT(c,t,false))return false;
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
    BC_COUNT(9);
    e.x=max(e.x,MIN);
    if(e.x>=e.y)return 1.;
#ifdef BC_ROW_BOUND
    if(bc_row_clear(cell,o,dir,e))return 1.;
#endif
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
#ifdef BC_PATCH_REJECT
    if(bc_patch_clear(local_patch,ua,ub,dir.z*e.x,dir.z*e.y))return 1.;
#endif
    float c[7];bc_ray_polynomial(local_patch,ua,ub,dir.z*e.x,dir.z*e.y,c);
#ifdef BC_PROFILE_COUNTERS
    bc_profile_task(true);
#endif
    // Visibility needs existence, not the earliest contact or a shading normal.
    // A strictly solid endpoint certifies occlusion by continuity. Exact-zero
    // endpoints retain the isolator's existing tangent / outgoing semantics.
    if(c[0]>0.||c[6]>0.){BC_COUNT(12);
#ifdef BC_PROFILE_COUNTERS
        bc_profile_shadow_fast(6);
#endif
        return 0.;}
    float high=c[0];for(int k=1;k<7;k++)high=max(high,c[k]);
    if(high<0.){BC_COUNT(13);
#ifdef BC_PROFILE_COUNTERS
        bc_profile_shadow_fast(7);
#endif
        return 1.;}
#if defined(BC_ANY_ROOT_EARLY) && !defined(BC_ROOT_STACK_LEFT_CONTINUE)
    return BC_ANY_ROOT(c)?0.:1.;
#else
    float root;return BC_FIRST_ROOT(c,root,true)?0.:1.;
#endif
}
