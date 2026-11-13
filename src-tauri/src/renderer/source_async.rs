impl GlRenderer {
    /// Sample zero remains on the verified path. Later pixels advance their own
    /// cursor only after their ordered fixed-function blend has been submitted.
    unsafe fn render_source_async_samples(&mut self,e:&mut PagedExport,cancel:&AtomicBool,remaining:u32)->Result<u32,String>{
        if self.count!=1||remaining==0{return Err("独立像素采样必须从已完成的首样本开始".into());}
        let target=remaining+1;
        let fused=std::env::var("DEMSTUDIO_ASYNC_FUSED_COMMIT").as_deref()==Ok("on");
        let queued=e.async_active_queue_enabled;
        let packed=std::env::var("DEMSTUDIO_ASYNC_PACKED_QUEUE").as_deref()==Ok("on");
        if packed&&!queued{return Err("PackedAsyncQueue需要AsyncActiveQueue=on".into());}
        if queued{
            crate::renderer::async_active_queue::validate_mode(self.geo_tile.is_some()&&self.output_tile.is_none(),e.image_integrator,true,fused,e.async_phase_pair.is_some(),e.wavefront.is_some(),packed)?;
            if e.async_active_queue.is_none(){e.async_active_queue=Some(crate::renderer::async_active_queue::AsyncActiveQueue::create(&self.gl)?);}
        }
        let size=[self.params.width,self.params.height];let total=size[0]*size[1];
        let params=self.params.clone();
        let table=self.generate_async_sample_table(&params,target);
        if let Some(samples)=e.async_samples.as_ref(){
            samples.update_table(&self.gl,&table.values,table.stride as u32,target)?;
        }else{
            e.async_samples=crate::renderer::async_samples::AsyncSamples::create(&self.gl,65536,target,&table.values,table.stride as u32)?;
        }
        if e.async_samples.is_none(){return Err("当前GPU不支持独立像素采样所需的双源混合与存储缓冲".into());}
        e.async_samples.as_ref().unwrap().reset(&self.gl,size,target)?;
        let activity_mode=std::env::var("DEMSTUDIO_DRAW_ACTIVITY").unwrap_or_else(|_|"sample".into());
        let activity_enabled=activity_mode!="off"&&e.batch.is_some();let mut activity_measured=false;
        if activity_enabled{e.batch.as_ref().unwrap().reset(&self.gl,0);}
        let path=e.async_path.unwrap();let original=e.path;
        let mut restore_bindings=true;let mut st_captured=false;let mut completed_before=0u32;let mut steps=1u32;
        let mut result=(||{
            loop {
                cancelled(cancel)?;
                if restore_bindings {
                    let paths=e.async_fresh_pair.or(e.async_phase_pair).unwrap_or([path,path]);
                    for program in paths.into_iter().take(if e.async_fresh_pair.is_some()||e.async_phase_pair.is_some(){2}else{1}){e.path=program;let prepared=self.prepare_source_pass(e,false);e.path=original;
                    self.restore_sample_rng(table.after_rng);prepared?;}
                    if queued{
                        if !st_captured{
                            let capture_timer=e.timers.begin(&self.gl,GpuStage::Memo);
                            e.async_active_queue.as_ref().unwrap().capture_st(&self.gl,original,size)?;
                            e.timers.end(&self.gl,GpuStage::Memo,capture_timer);st_captured=true;
                        }
                        self.gl.use_program(Some(path));e.async_active_queue.as_ref().unwrap().bind_st(&self.gl,path);
                    }
                    restore_bindings=false;
                }
                let started=std::time::Instant::now();
                let timer=e.timers.begin(&self.gl,GpuStage::Batch);
                for _ in 0..steps {
                    cancelled(cancel)?;
                    if activity_enabled&&crate::renderer::async_samples::measure_activity(&activity_mode,!activity_measured,sample_draw_activity(e.stats.draws)){
                        let activity_timer=e.timers.begin(&self.gl,GpuStage::BatchController);
                        e.async_samples.as_ref().unwrap().record_activity(&self.gl,e.batch.as_ref().unwrap(),e.state[0],e.page_table)?;
                        e.timers.end(&self.gl,GpuStage::BatchController,activity_timer);
                        activity_measured=true;
                    }
                    self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));
                    let paths=e.async_fresh_pair.or(e.async_phase_pair).unwrap_or([path,path]);
                    let pass_count=if e.async_fresh_pair.is_some()||e.async_phase_pair.is_some(){2}else{1};
                    for (pass,program)in paths.into_iter().take(pass_count).enumerate(){
                        let emits=fused&&(e.async_fresh_pair.is_some()||pass+1==pass_count);
                        if emits{
                            self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);self.gl.enable(glow::BLEND);
                            self.gl.blend_equation(glow::FUNC_ADD);self.gl.blend_func(glow::SRC1_ALPHA,glow::ONE_MINUS_SRC1_ALPHA);
                        }else{self.gl.draw_buffers(&[glow::NONE]);self.gl.disable(glow::BLEND);}
                        self.gl.bind_image_texture(0,Some(e.state[0]),0,true,0,glow::READ_WRITE,glow::RGBA32F);
                        self.gl.use_program(Some(program));e.async_samples.as_ref().unwrap().bind(&self.gl,program);
                        self.i(program,"rs_use_point_queue",0);
                        if queued{
                            let queue_timer=e.timers.begin(&self.gl,GpuStage::BatchQueue);
                            e.async_active_queue.as_ref().unwrap().dispatch(&self.gl,e.state[0],e.page_table,size,target)?;
                            e.timers.end(&self.gl,GpuStage::BatchQueue,queue_timer);
                            e.async_queue_builds+=1;
                        }
                        let integration_timer=e.timers.begin(&self.gl,GpuStage::BatchIntegrate);
                        if queued&&!packed{e.async_active_queue.as_ref().unwrap().draw(&self.gl,program,size)?;}else{self.gl.draw_arrays(glow::TRIANGLES,0,3);}
                        e.stats.draws+=1;e.async_draws+=1;
                        
                        #[cfg(not(test))] self.gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);
                        e.timers.end(&self.gl,GpuStage::BatchIntegrate,integration_timer);
                    }
                    if fused {self.gl.disable(glow::BLEND);}else{
                        let commit_timer=e.timers.begin(&self.gl,GpuStage::BatchCommit);self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
                        e.async_samples.as_ref().unwrap().commit(&self.gl,e.state[0],size)?;e.timers.end(&self.gl,GpuStage::BatchCommit,commit_timer);
                    }
                    self.gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT|glow::FRAMEBUFFER_BARRIER_BIT);
                }
                e.stats.draw_submission_seconds+=started.elapsed().as_secs_f64();
                let reduction_timer=e.timers.begin(&self.gl,GpuStage::BatchReduction);
                e.compact.as_mut().ok_or("独立像素采样需要紧凑反馈")?.dispatch(&self.gl,e.state[0],size)?;
                e.timers.end(&self.gl,GpuStage::BatchReduction,reduction_timer);
                let controller_timer=e.timers.begin(&self.gl,GpuStage::BatchController);
                e.async_samples.as_ref().unwrap().dispatch_progress(&self.gl)?;
                e.timers.end(&self.gl,GpuStage::BatchController,controller_timer);
                e.timers.end(&self.gl,GpuStage::Batch,timer);
                let waiting=std::time::Instant::now();
                let feedback=e.compact.as_mut().unwrap().read(&self.gl)?;
                let progress=e.async_samples.as_ref().unwrap().read(&self.gl)?;
                e.stats.feedback_reads+=2;e.batch_syncs+=1;
                e.stats.feedback_bytes+=e.compact.as_ref().unwrap().readback_bytes() as u64+16;
                let waited=waiting.elapsed().as_secs_f64();e.stats.feedback_seconds+=waited;
                e.max_batch_wait_seconds=e.max_batch_wait_seconds.max(waited);
                if feedback.invalid!=0||progress.invalid!=0||progress.completed_samples<completed_before {
                    return Err("独立像素采样状态无效".into());
                }
                let delta=progress.completed_samples-completed_before;completed_before=progress.completed_samples;
                e.stats.pixel_samples+=u64::from(delta);e.async_completed+=u64::from(delta);
                let elapsed=started.elapsed().as_secs_f64();e.max_batch_wall_seconds=e.max_batch_wall_seconds.max(elapsed);
                // The existing async policy uses this observed wall interval;
                // it does not attempt to consume an asynchronous timestamp result.
                e.budget_wall_fallbacks+=1;e.budget_async_wall_observations+=1;
                if !feedback.requests.is_empty(){
                    e.stats.page_fault_rounds+=1;e.stats.page_fault_requests+=feedback.requests.len() as u64;
                    let resident:Vec<_>=e.slots.iter().filter_map(|&page|page).collect();
                    let ranked=e.scheduler.rank(&feedback.requests,&resident);e.load_pages(&self.gl,&ranked)?;
                    restore_bindings=true;steps=1;
                }else{
                    let budget=std::env::var("DEMSTUDIO_GPU_BATCH_MS").ok().and_then(|s|s.parse::<f64>().ok()).filter(|x|x.is_finite()).unwrap_or(50.).clamp(5.,200.)/1000.;
                    steps=if elapsed>budget{(steps/2).max(1)}else if elapsed<budget/2.{(steps*2).min(32)}else{steps};
                }
                e.timers.collect(&self.gl,false);e.checkpoint(&self.gl,"independent_pixel_samples",false);
                if progress.finished_pixels==total {
                    if completed_before!=total*remaining{return Err("独立像素采样累计数量不一致".into());}
                    self.count=target;self.output_dirty.set(true);return self.check("独立像素采样").map(|_|remaining);
                }
            }
        })();
        if activity_measured{
            let waiting=std::time::Instant::now();let telemetry=e.batch.as_ref().unwrap().read(&self.gl);
            e.stats.feedback_reads+=1;e.stats.feedback_bytes+=64;let waited=waiting.elapsed().as_secs_f64();
            e.stats.feedback_seconds+=waited;e.max_batch_wait_seconds=e.max_batch_wait_seconds.max(waited);
            match telemetry{Ok(t)=>{e.activity_active+=u64::from(t.eligible_invocations);e.activity_capacity+=u64::from(t.eligible_capacity);
                e.activity_draws+=u64::from(t.draws);e.activity_zero+=u64::from(t.zero_draws);},Err(error)=>{if result.is_ok(){result=Err(error);}}}
        }
        e.path=original;self.restore_sample_rng(table.after_rng);
        self.gl.disable(glow::BLEND);self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
        result
    }
}
