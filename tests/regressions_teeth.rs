use lust::regex::BoundedRegex;
#[test]
fn find_at_eof_must_not_panic() {
 let re=BoundedRegex::new("z").unwrap();
 let found=re.find("z").unwrap().unwrap();
 assert_eq!((found.start,found.end),(0,1));
}
#[test]
fn find_empty_input_must_not_panic() {
 let re=BoundedRegex::new("a*").unwrap();
 assert!(re.find("").unwrap().is_some());
}
#[test]
fn replace_no_match_scaling() {
 let re=BoundedRegex::new("a*z").unwrap();
 for n in [2000,4000,8000,16000] {
  let s="a".repeat(n);
  let start=std::time::Instant::now();
  assert_eq!(re.replace(&s,"x").unwrap(),s);
  eprintln!("replace n={n} elapsed={:?}",start.elapsed());
 }
}

#[test]
fn duplicate_ordinals_must_not_lose_data() {
 use lust::data::{DataValue, Document, ExactNumber};
 use lust::data::layout::{LogicalFieldDesc,LogicalLayoutV1,CanonicalTypeTag};
 use lust::data::materialize::materialize_from_document;
 let layout=LogicalLayoutV1::new(42,"Duplicate",vec![
 LogicalFieldDesc::new(0,"a",CanonicalTypeTag::Int64),
 LogicalFieldDesc::new(0,"b",CanonicalTypeTag::Int64)]);
 let mut doc=Document::new();
 doc.try_insert("a",DataValue::Number(ExactNumber::Int(1))).unwrap();
 doc.try_insert("b",DataValue::Number(ExactNumber::Int(2))).unwrap();
 if let Ok(value)=materialize_from_document(&layout,&doc) {
  assert_eq!(value.to_document(),doc,"accepted layout must not silently overwrite distinct fields");
 }
}
