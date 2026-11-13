layout(local_size_x=256) in;
layout(rgba32f,binding=0) uniform image2DArray rs_state_image;
layout(std430,binding=1) readonly buffer RayStateActiveQueue { uint rs_active_words[]; };
uniform ivec2 rs_extent;
uniform int rs_use_active_queue;
uniform vec2 aspect;
vec2 rs_fragcoord;
