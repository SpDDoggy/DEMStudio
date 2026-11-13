// Deterministic hashed sampling avoids aliasing with a fixed 8-Draw phase cycle.
fn sample_draw_activity(draw:u64)->bool {
    let mut x=draw.wrapping_add(0x9e3779b97f4a7c15);
    x=(x^(x>>30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x=(x^(x>>27)).wrapping_mul(0x94d049bb133111eb);
    (x^(x>>31))&63==0
}
impl GlRenderer {
    pub unsafe fn render_source_batch(&mut self,e:&mut PagedExport,cancel:&AtomicBool,remaining:u32)->Result<u32,String>{
        if self.count==0||e.batch.is_none()||self.geo_tile.is_none() {
            self.render_source_sample(e,cancel)?;return Ok(1);
        }
        if std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4"){return self.render_source_wavefront(e,cancel,remaining);}
        if e.async_path.is_some(){return self.render_source_async_samples(e,cancel,remaining);}
        let base=self.count;let size=[self.params.width,self.params.height];let total=size[0]*size[1];
        e.batch.as_ref().unwrap().reset_resume(&self.gl,base,e.batch_resume);
        let mut telemetry=[0u32;8];
        // This call owns one frozen geographic tile. The base prepare below
        // restores shared bindings every candidate; phase programs retain their
        // static uniforms and need only the identical sample's random uniforms.
        // Limit reuse to this call, including its page-fault retries, so later
        // tiles/parameters always receive a full phase-program initialization.
        let phase_params=self.params.clone();
        let mut phase_uniforms_ready=false;
        let result=(||{
            loop {
                cancelled(cancel)?;
                let started=std::time::Instant::now();let candidates=e.batch_samples.min(remaining).max(1);
                let mut checkpoints=vec![self.sample_rng_checkpoint()];
                let activity_mode=std::env::var("DEMSTUDIO_DRAW_ACTIVITY").unwrap_or_else(|_|"sample".into());
                let activity=activity_mode=="on"&&e.active_queue.is_some()&&!e.compute;
                let timer=e.timers.begin(&self.gl,GpuStage::Batch);
                for sample in base..base+candidates {
                    cancelled(cancel)?;self.count=sample;
                    let before=self.sample_rng_checkpoint();self.prepare_source_pass(e,false)?;
                    if let Some(paths)=e.phase_paths {
                        let after=self.sample_rng_checkpoint();let original=e.path;
                        for program in paths {
                            self.restore_sample_rng(before);
                            if phase_uniforms_ready {
                                self.gl.use_program(Some(program));
                                self.advance_sample_uniforms(program,&phase_params);
                            }else{
                                e.path=program;
                                let prepared=self.prepare_source_pass(e,false);e.path=original;
                                if let Err(error)=prepared {self.restore_sample_rng(after);return Err(error);}
                            }
                        }
                        phase_uniforms_ready=true;
                        self.restore_sample_rng(after);
                    }
                    checkpoints.push(self.sample_rng_checkpoint());
                    let controller_timer=e.timers.begin(&self.gl,GpuStage::BatchController);
                    e.batch.as_ref().unwrap().begin_candidate(&self.gl,sample);
                    e.timers.end(&self.gl,GpuStage::BatchController,controller_timer);
                    if let Some(paths)=e.phase_paths {
                        self.gl.use_program(Some(paths[0]));self.i(paths[0],"rs_batch_mode",1);
                        self.i(paths[0],"rs_expected_sample",sample as i32);self.i(paths[0],"rs_use_point_queue",0);
                        self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));self.gl.draw_buffers(&[glow::NONE]);
                        self.gl.bind_image_texture(0,Some(e.state[0]),0,true,0,glow::READ_WRITE,glow::RGBA32F);
                        self.gl.draw_arrays(glow::TRIANGLES,0,3);e.stats.draws+=1;
                        self.gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);
                    }
                    for step in 0..e.batch_steps {
                        if let Some(paths)=e.phase_paths {
                            let rounds=std::env::var("DEMSTUDIO_PHASE_ROUNDS").ok().and_then(|v|v.parse::<u32>().ok()).unwrap_or(4).clamp(1,8);
                            for _ in 0..rounds {
                                cancelled(cancel)?;
                                let queue=e.phase_queue.as_ref().unwrap();
                                // Classification plus its device-only four-command activity
                                // reduction; never scan state again or add a CPU readback.
                                let queue_timer=e.timers.begin(&self.gl,GpuStage::BatchQueue);
                                queue.dispatch(&self.gl,e.state[0],e.page_table,e.size,e.source.logical_size[0],Some(sample))?;
                                if activity_mode!="off" {e.batch.as_ref().unwrap().record_phase_queue(&self.gl,sample,total);}
                                e.timers.end(&self.gl,GpuStage::BatchQueue,queue_timer);
                                self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));self.gl.draw_buffers(&[glow::NONE]);
                                self.gl.bind_image_texture(0,Some(e.state[0]),0,true,0,glow::READ_WRITE,glow::RGBA32F);
                                for (index,phase) in crate::renderer::phase_queue::Phase::ALL.into_iter().enumerate() {
                                    let program=paths[index+1];self.gl.use_program(Some(program));
                                    self.i(program,"rs_batch_mode",1);self.i(program,"rs_expected_sample",sample as i32);
                                    if e.phase_compute {
                                        queue.dispatch_phase(&self.gl,program,e.size,phase)?;
                                    }else{
                                        queue.draw(&self.gl,program,e.size,phase)?;e.stats.draws+=1;
                                    }
                                }
                                self.gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);
                                e.phase_rounds+=1;
                            }
                            continue;
                        }
                        cancelled(cancel)?;
                        let measure=activity||(activity_mode!="off"&&e.active_queue.is_some()&&(e.activity_draws==0||sample_draw_activity(e.stats.draws)));
                        if measure {
                            let queue_timer=e.timers.begin(&self.gl,GpuStage::BatchQueue);
                            e.active_queue.as_ref().unwrap().dispatch_batch(&self.gl,e.state[0],e.page_table,e.size,e.source.logical_size[0],step>0,false,false,sample)?;
                            e.batch.as_ref().unwrap().record_queue_draw(&self.gl,sample,total);
                            e.timers.end(&self.gl,GpuStage::BatchQueue,queue_timer);
                        }
                        let integration_timer=e.timers.begin(&self.gl,GpuStage::BatchIntegrate);
                        self.gl.use_program(Some(e.path));self.i(e.path,"rs_batch_mode",1);self.i(e.path,"rs_expected_sample",sample as i32);
                        self.i(e.path,"pg_resume",i32::from(step>0));self.i(e.path,"rs_use_point_queue",i32::from(activity&&step>0));
                        self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));self.gl.draw_buffers(&[glow::NONE]);
                        self.gl.bind_image_texture(0,Some(e.state[0]),0,true,0,glow::READ_WRITE,glow::RGBA32F);
                        if e.compute {
                            self.i(e.path,"rs_use_active_queue",0);
                            self.gl.uniform_2_i32(self.gl.get_uniform_location(e.path,"rs_extent").as_ref(),e.size[0]as i32,e.size[1]as i32);
                            self.gl.dispatch_compute((e.size[0]*e.size[1]).div_ceil(crate::renderer::raystate_shader::compute_group_size()),1,1);
                        }
                        else if activity&&step>0 {e.active_queue.as_ref().unwrap().draw(&self.gl,e.path,e.size)?;}
                        else {self.draw_source_image(e)?;}
                        self.gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);e.stats.draws+=1;
                        e.timers.end(&self.gl,GpuStage::BatchIntegrate,integration_timer);
                    }
                    let reduction_timer=e.timers.begin(&self.gl,GpuStage::BatchReduction);
                    let compact=e.compact.as_mut().unwrap();compact.dispatch(&self.gl,e.state[0],size)?;
                    e.batch_reductions+=1;
                    e.timers.end(&self.gl,GpuStage::BatchReduction,reduction_timer);
                    let controller_timer=e.timers.begin(&self.gl,GpuStage::BatchController);
                    e.batch.as_ref().unwrap().finish_candidate(&self.gl,sample,total);
                    if sample+1<base+candidates {compact.consumed_on_gpu();}
                    e.timers.end(&self.gl,GpuStage::BatchController,controller_timer);
                    let commit_timer=e.timers.begin(&self.gl,GpuStage::BatchCommit);
                    self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
                    self.gl.use_program(Some(e.commit));self.i(e.commit,"rs_batch_mode",1);self.i(e.commit,"rs_expected_sample",sample as i32);
                    e.bind_array(&self.gl,e.commit,"state_image",6,e.state[0]);
                    self.gl.enable(glow::BLEND);let alpha=1./(sample+1) as f32;
                    self.gl.blend_color(alpha,alpha,alpha,alpha);self.gl.blend_equation(glow::FUNC_ADD);
                    self.gl.blend_func(glow::CONSTANT_ALPHA,glow::ONE_MINUS_CONSTANT_ALPHA);self.gl.draw_arrays(glow::TRIANGLES,0,3);self.gl.disable(glow::BLEND);
                    e.timers.end(&self.gl,GpuStage::BatchCommit,commit_timer);
                    let controller_timer=e.timers.begin(&self.gl,GpuStage::BatchController);
                    e.batch.as_ref().unwrap().advance(&self.gl,sample);
                    e.timers.end(&self.gl,GpuStage::BatchController,controller_timer);
                }
                e.stats.draw_submission_seconds+=started.elapsed().as_secs_f64();
                let waiting=std::time::Instant::now();
                // The final candidate already reduced this unchanged state.
                e.timers.end(&self.gl,GpuStage::Batch,timer);
                let (feedback,control)=e.compact.as_mut().unwrap().read_batch(&self.gl,e.batch.as_ref().unwrap())?;let elapsed=started.elapsed().as_secs_f64();
                e.stats.feedback_reads+=1;e.batch_syncs+=1;e.stats.feedback_bytes+=64+e.compact.as_ref().unwrap().readback_bytes() as u64;
                let waited=waiting.elapsed().as_secs_f64();
                e.stats.feedback_seconds+=waited;e.max_batch_wait_seconds=e.max_batch_wait_seconds.max(waited);
                e.max_batch_wall_seconds=e.max_batch_wall_seconds.max(elapsed);
                e.timers.collect(&self.gl,false);
                let gpu_elapsed=match e.timers.interval_seconds(GpuStage::Batch,timer){
                    Some(seconds)=>{e.budget_gpu_observations+=1;seconds},
                    None=>{e.budget_wall_fallbacks+=1;elapsed}
                };
                if control.cursor<base||control.cursor>base+candidates||control.invalid!=0||feedback.invalid!=0 {return Err(format!("GPU采样批次状态无效: base={base} candidates={candidates} control={control:?} feedback_invalid={}",feedback.invalid));}
                let current=[control.boundary_done,control.boundary_waiting,control.boundary_active,control.boundary_capacity,
                    control.eligible_invocations,control.eligible_capacity,control.draws,control.zero_draws];
                let delta:Vec<u64>=current.iter().zip(telemetry).map(|(&now,last)|u64::from(now.saturating_sub(last))).collect();
                e.boundary_done+=delta[0];e.boundary_waiting+=delta[1];e.boundary_active+=delta[2];e.boundary_capacity+=delta[3];
                e.activity_active+=delta[4];e.activity_capacity+=delta[5];e.activity_draws+=delta[6];e.activity_zero+=delta[7];telemetry=current;
                let completed=control.cursor-base;self.restore_sample_rng(checkpoints[completed as usize]);self.count=control.cursor;
                e.batch_resume=control.initialized;
                let target=std::env::var("DEMSTUDIO_GPU_BATCH_MS").ok().and_then(|v|v.parse::<f64>().ok()).filter(|v|v.is_finite()).unwrap_or(50.).clamp(5.,200.)/1000.;
                if !feedback.requests.is_empty()&&completed<candidates {
                    e.stats.page_fault_rounds+=1;e.stats.page_fault_requests+=feedback.requests.len() as u64;
                    let resident:Vec<_>=e.slots.iter().filter_map(|&page|page).collect();let ranked=e.scheduler.rank(&feedback.requests,&resident);
                    e.load_pages(&self.gl,&ranked)?;e.batch_samples=1;
                } else if completed<candidates {
                    // Incomplete work must not bypass the execution-time budget.
                    e.batch_steps=if gpu_elapsed>target {(e.batch_steps/2).max(1)}
                        else if gpu_elapsed<target/2. {(e.batch_steps*2).min(8)}else{e.batch_steps};
                    e.batch_samples=1;
                }
                else if gpu_elapsed>target {e.batch_samples=(e.batch_samples/2).max(1);e.batch_steps=(e.batch_steps/2).max(1);}
                else if completed==candidates&&gpu_elapsed<target/2. {e.batch_samples=(e.batch_samples*2).min(32);}
                if completed>0 {
                    e.stats.pixel_samples+=u64::from(total)*u64::from(completed);e.batch_completed+=u64::from(completed);
                    self.output_dirty.set(true);e.checkpoint(&self.gl,"sample_batch_complete",false);return self.check("GPU采样批次").map(|_|completed);
                }
                e.checkpoint(&self.gl,"sample_batch_waiting",false);
            }
        })();
        self.gl.use_program(Some(e.path));self.i(e.path,"rs_batch_mode",0);
        self.gl.use_program(Some(e.commit));self.i(e.commit,"rs_batch_mode",0);
        self.gl.disable(glow::BLEND);self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);result
    }
}
