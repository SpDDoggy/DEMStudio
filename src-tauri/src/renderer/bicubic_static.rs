//! Experimental source expansion of fixed-size Bicubic local-array loops.
//! The DFS stack pointer and interval-depth loops remain dynamic.
pub(super) fn expand(source:&str)->String {
    // bc_bound has continue statements: keep its footprint traversal unchanged.
    let Some(start)=source.find("void bc_patch(")else{return source.into();};
    let mut result=source[..start].to_owned();
    result.push_str(&expand_fixed(&source[start..]));result
}
fn constant(expression:&str)->Option<usize>{
    if let Some((a,b))=expression.split_once('-'){a.parse::<usize>().ok()?.checked_sub(b.parse().ok()?)}else{expression.parse().ok()}
}
fn replace_identifier(text:&str,name:&str,value:usize)->String {
    let mut out=String::new();let mut rest=text;
    while let Some(pos)=rest.find(name){
        let after=pos+name.len();
        let ident=|c:char|c.is_ascii_alphanumeric()||c=='_';
        if rest[..pos].chars().next_back().is_some_and(|c|ident(c)||c=='.')||rest[after..].chars().next().is_some_and(ident){out.push_str(&rest[..after]);rest=&rest[after..];continue;}
        out.push_str(&rest[..pos]);out.push_str(&value.to_string());rest=&rest[after..];
    }
    out.push_str(rest);out
}
fn expand_fixed(source:&str)->String {
    let mut out=String::new();let mut rest=source;
    while let Some(start)=rest.find("for(int ") {
        let header_start=start+8;
        let Some(close)=rest[header_start..].find(')')else{break;};let close=header_start+close;
        let header=&rest[header_start..close];let fields:Vec<_>=header.split(';').collect();
        let parsed=(||{
            let [init,condition,step]=fields.as_slice()else{return None;};
            let (name,initial)=init.split_once('=')?;
            if !["x","y","j","k","d"].contains(&name){return None;}
            let first=initial.parse::<usize>().ok()?;
            let end=constant(condition.strip_prefix(&format!("{name}<"))?)?;
            if *step!=format!("{name}++")||first>end||end>16{return None;}
            Some((name,first,end))
        })();
        let Some((name,first,end))=parsed else{out.push_str(&rest[..close+1]);rest=&rest[close+1..];continue;};
        let body_start=close+1+rest[close+1..].len()-rest[close+1..].trim_start().len();
        let body_end=if rest[body_start..].starts_with('{'){
            let mut depth=0;let mut end=None;
            for (offset,ch) in rest[body_start..].char_indices(){if ch=='{'{depth+=1;}else if ch=='}'{depth-=1;if depth==0{end=Some(body_start+offset+1);break;}}}
            let Some(end)=end else{break;};end
        }else{let Some(end)=rest[body_start..].find(';')else{break;};body_start+end+1};
        let body=&rest[body_start..body_end];out.push_str(&rest[..start]);
        // Each iteration keeps its own scope, with the same statement order.
        out.push_str("{\n");
        for value in first..end {out.push_str("{\n");out.push_str(&expand_fixed(&replace_identifier(body,name,value)));out.push_str("\n}\n");}
        out.push('}');rest=&rest[body_end..];
    }
    out.push_str(rest);out
}



