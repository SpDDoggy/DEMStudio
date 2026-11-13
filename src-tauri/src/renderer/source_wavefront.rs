impl GlRenderer {
    /// Four real samples per block; source sample zero remains on the old path.
    unsafe fn render_source_wavefront(&mut self,e:&mut PagedExport,cancel:&AtomicBool,remaining:u32)->Result<u32,String>{
        if self.count!=1||remaining==0||self.geo_tile.is_none()||self.params.surface!="bilinear"||!e.image_integrator{
            return Err("四路样本候选要求Bilinear地理image积分器且首样本已完成".into());
        }
        let target=remaining.checked_add(1).filter(|v|*v<=256).ok_or("wavefront samples exceed 256")?;
        let pair=std::env::var("DEMSTUDIO_WAVEFRONT_PAIR").as_deref()==Ok("on");
        let fused=std::env::var("DEMSTUDIO_WAVEFRONT_FUSED").as_deref()==Ok("on");
        let size=[self.params.width,self.params.height];let params=self.params.clone();
        let table=self.generate_async_sample_table(&params,target);let original=e.path;let draws_before=e.stats.draws;
        let result=(||{
            cancelled(cancel)?;
            if e.wavefront.is_none(){
                let dense=path_source(&params,true);let mut programs=Vec::new();
                for stage in 0..5{
                    let made=(||{let sources=if fused&&stage>0{crate::renderer::sample_wavefront_shader::source_fused(&dense)?}else if pair&&stage==2{crate::renderer::sample_wavefront_shader::source_other(&dense)?}else{crate::renderer::sample_wavefront_shader::source(&dense,stage)?};program_with_vertex(&self.gl,&sources.vertex,&sources.fragment)})();
                    match made{Ok(p)=>programs.push(p),Err(error)=>{for p in programs{self.gl.delete_program(p);}return Err(error);}}
                }
                let programs:[glow::Program;5]=programs.try_into().map_err(|_|"wavefront program count invalid")?;
                e.wavefront=Some(crate::renderer::wavefront_runtime::Runtime::create(&self.gl,programs,&table.values,table.stride as u32,target)?);
            }else{e.wavefront.as_ref().unwrap().update_table(&self.gl,&table.values,table.stride as u32,target)?;}
            e.wavefront.as_mut().unwrap().ensure_layout(&self.gl,size)?;
            if e.wavefront.as_ref().unwrap().memory_bytes()>e.wavefront_reserved_bytes{return Err("wavefront allocation exceeds joint reservation".into());}
            let layout=e.wavefront.as_ref().unwrap().resources.as_ref().unwrap().layout;
            let alphas:Vec<f32>=(0..target).map(|sample|table.values[sample as usize*table.stride+table.stride-3]).collect();
            let mut base=1;let mut restore_bindings=true;let mut steps=1u32;
            while let Some(block)=layout.block(base,target){
                cancelled(cancel)?;
                if restore_bindings{
                    let programs=e.wavefront.as_ref().unwrap().programs;
                    for program in programs{e.path=program;let prepared=self.prepare_source_pass(e,false);e.path=original;self.restore_sample_rng(table.after_rng);prepared?;}
                    restore_bindings=false;
                }
                let state=e.wavefront.as_ref().unwrap().resources.as_ref().unwrap().state;
                self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));self.gl.draw_buffers(&[glow::NONE]);self.gl.disable(glow::BLEND);
                self.gl.bind_image_texture(0,Some(state),0,true,0,glow::READ_WRITE,glow::RGBA32F);
                let timer=e.timers.begin(&self.gl,GpuStage::BatchIntegrate);
                {let runtime=e.wavefront.as_ref().unwrap();let program=runtime.programs[0];runtime.bind(&self.gl,program,block,e.state[0]);
                    for lane in 0..block.count{cancelled(cancel)?;self.gl.uniform_1_u32(self.gl.get_uniform_location(program,"sw_draw_lane").as_ref(),lane);self.gl.draw_arrays(glow::TRIANGLES,0,3);e.stats.draws+=1;}
                }
                e.timers.end(&self.gl,GpuStage::BatchIntegrate,timer);
                self.gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
                loop{
                    cancelled(cancel)?;
                    if restore_bindings{
                        let programs=e.wavefront.as_ref().unwrap().programs;
                        for program in programs{e.path=program;let prepared=self.prepare_source_pass(e,false);e.path=original;self.restore_sample_rng(table.after_rng);prepared?;}
                        restore_bindings=false;
                    }
                    let started=std::time::Instant::now();let batch_timer=e.timers.begin(&self.gl,GpuStage::Batch);
                    for _ in 0..steps{
                        cancelled(cancel)?;
                        self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));self.gl.draw_buffers(&[glow::NONE]);self.gl.disable(glow::BLEND);
                        self.gl.bind_image_texture(0,Some(state),0,true,0,glow::READ_WRITE,glow::RGBA32F);
                        let queue_timer=e.timers.begin(&self.gl,GpuStage::BatchQueue);
                        let queue=e.wavefront.as_ref().unwrap().queue.as_ref().unwrap();
                        if fused{queue.dispatch_fused(&self.gl,state,e.page_table,e.source.logical_size[0],block)?;}
                        else if pair{queue.dispatch_pair(&self.gl,state,e.page_table,e.source.logical_size[0],block)?;}
                        else{queue.dispatch(&self.gl,state,e.page_table,e.source.logical_size[0],block)?;}
                        e.timers.end(&self.gl,GpuStage::BatchQueue,queue_timer);
                        let integrate=e.timers.begin(&self.gl,GpuStage::BatchIntegrate);
                        for (index,phase) in [crate::renderer::wavefront_queue::Phase::World,crate::renderer::wavefront_queue::Phase::Surface,crate::renderer::wavefront_queue::Phase::Shadow,crate::renderer::wavefront_queue::Phase::Scatter].into_iter().take(if fused{1}else if pair{2}else{4}).enumerate(){
                            let runtime=e.wavefront.as_ref().unwrap();let program=runtime.programs[index+1];runtime.bind(&self.gl,program,block,e.state[0]);
                            for segment in 0..layout.paths.div_ceil(crate::renderer::sample_wavefront::SEGMENT_PATHS){cancelled(cancel)?;runtime.queue.as_ref().unwrap().draw_segment(&self.gl,program,phase,segment)?;e.stats.draws+=1;}
                        }
                        e.timers.end(&self.gl,GpuStage::BatchIntegrate,integrate);e.phase_rounds+=1;
                        self.gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
                    }
                    e.stats.draw_submission_seconds+=started.elapsed().as_secs_f64();
                    let reduction=e.timers.begin(&self.gl,GpuStage::BatchReduction);
                    e.compact.as_mut().ok_or("wavefront requires compact feedback")?.dispatch(&self.gl,state,[size[0],size[1]*block.count])?;
                    e.timers.end(&self.gl,GpuStage::BatchReduction,reduction);e.timers.end(&self.gl,GpuStage::Batch,batch_timer);
                    let waiting=std::time::Instant::now();
                    let(feedback,diagnostic)=e.compact.as_mut().unwrap().read_wavefront(&self.gl,e.wavefront.as_ref().unwrap().queue.as_ref().unwrap())?;
                    e.stats.feedback_reads+=1;e.batch_syncs+=1;e.stats.feedback_bytes+=e.compact.as_ref().unwrap().readback_bytes()as u64+32;e.stats.feedback_seconds+=waiting.elapsed().as_secs_f64();
                    if feedback.invalid!=0||diagnostic[..3].iter().any(|&v|v!=0)||feedback.total!=layout.pixels*block.count{return Err("wavefront state or queue invalid".into());}
                    let elapsed=started.elapsed().as_secs_f64();e.max_batch_wall_seconds=e.max_batch_wall_seconds.max(elapsed);
                    if !feedback.requests.is_empty(){
                        e.stats.page_fault_rounds+=1;e.stats.page_fault_requests+=feedback.requests.len()as u64;
                        let resident:Vec<_>=e.slots.iter().filter_map(|&p|p).collect();let ranked=e.scheduler.rank(&feedback.requests,&resident);e.load_pages(&self.gl,&ranked)?;restore_bindings=true;steps=1;
                    }else{
                        let budget=std::env::var("DEMSTUDIO_GPU_BATCH_MS").ok().and_then(|s|s.parse::<f64>().ok()).filter(|x|x.is_finite()).unwrap_or(50.).clamp(5.,200.)/1000.;
                        steps=if elapsed>budget{(steps/2).max(1)}else if elapsed<budget/2.{(steps*2).min(32)}else{steps};
                    }
                    e.timers.collect(&self.gl,false);e.checkpoint(&self.gl,"sample_wavefront",false);
                    if feedback.all_paths_done(){break;}
                }
                cancelled(cancel)?;self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);self.gl.viewport(0,0,size[0]as i32,size[1]as i32);
                let commit=e.timers.begin(&self.gl,GpuStage::BatchCommit);e.wavefront.as_ref().unwrap().resources.as_ref().unwrap().commit_ordered(&self.gl,block,&alphas)?;e.timers.end(&self.gl,GpuStage::BatchCommit,commit);
                e.stats.pixel_samples+=u64::from(layout.pixels)*u64::from(block.count);e.batch_completed+=u64::from(block.count);
                let runtime=e.wavefront.as_mut().unwrap();runtime.blocks+=1;runtime.committed_samples+=u64::from(block.count);
                base+=block.count;self.count=base;self.output_dirty.set(true);
            }
            self.check("四路样本wavefront").map(|_|remaining)
        })();
        if let Some(runtime)=e.wavefront.as_mut(){runtime.integration_draws+=e.stats.draws-draws_before;}
        e.path=original;self.restore_sample_rng(table.after_rng);self.gl.disable(glow::BLEND);self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);result
    }
}

