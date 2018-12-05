
extern crate batsmt_core;

mod ast {
    use batsmt_core::ast::*;

    #[test]
    fn test_mk_const() {
        let mut m = AstManager::new();
        let a = m.mk_const("a");
        let b = m.mk_const("b");
        assert_ne!(a,b);
        let a2 = m.mk_const("a");
        assert_ne!(a,a2);
    }

    #[test]
    fn test_view_const() {
        let mut m = AstManager::new();
        let a = m.mk_const("a");
        let b = m.mk_const("b");
        assert!(match m.view(a) { View::Const(s) => s.eq_str("a"), _ => false });
        assert!(match m.view(b) { View::Const(s) => s.eq_str("b"), _ => false });
    }

    #[test]
    fn test_mk_fun() {
        let mut m = AstManager::new();
        let f = m.mk_const("f");
        let g = m.mk_const("g");
        let a = m.mk_const("a");
        let b = m.mk_const("b");
        let t1 = m.mk_app(f, &[a,b]);
        let t2 = m.mk_app(f, &[a,b]);
        let t3 = m.mk_app(f, &[b,a]);
        let t4 = m.mk_app(g, &[t1]);
        let t5 = m.mk_app(g, &[t2]);
        assert_eq!(t1,t2);
        assert_ne!(t1,t3);
        assert_ne!(t2,t3);
        assert_eq!(t4,t5);

        let t10 = m.mk_app(f, &[a; 10]);
        let t11 = m.mk_app(f, &[a; 10]);
        let t12 = m.mk_app(f, &[b; 10]);
        assert_eq!(t10,t11);
        assert_ne!(t10,t12);
    }

    #[test]
    fn test_view() {
        let mut m = AstManager::new();
        let f = m.mk_const("f");
        let g = m.mk_const("g");
        let a = m.mk_const("a");
        let b = m.mk_const("b");
        let t1 = m.mk_app(f, &[a,b]);
        let t2 = m.mk_app(f, &[b,a]);
        let t4 = m.mk_app(g, &[t1]);
        assert!(match m.view(t1) { View::App{f:f2, args} => f2 == f && args==&[a,b], _ => false });
        assert!(match m.view(t2) { View::App{f:f2, args} => f2 == f && args==&[b,a], _ => false });
        assert!(match m.view(t4) { View::App{f:f2, args} => f2 == g && args==&[t1], _ => false });
        let t10 = m.mk_app(f, &[a; 10]);
        let t11 = m.mk_app(f, &[b; 10]);
        assert!(match m.view(t10) { View::App{f:f2, args} => f2 == f && args==&[a;10], _ => false });
        assert!(match m.view(t11) { View::App{f:f2, args} => f2 == f && args==&[b;10], _ => false });
    }

    fn mk_stress_app(n: usize, long_apps: bool) {
        use std::time::Instant;

        let mut m = AstManager::new();
        let f = m.mk_const("f");
        let g = m.mk_const("g");
        let a = m.mk_const("a");
        let b = m.mk_const("b");

        let mut n_app_created = 0;
        let start = Instant::now();
        {
            let mut terms = vec![a,b];
            // create a bunch of terms
            let mut i = 0;
            let mut tmp = vec![];
            while terms.len () < n {
                for &t1 in terms[i..].iter() {
                    for &t2 in terms.iter() {
                        let t = m.mk_app(f, &[t1,t2]);
                        tmp.push(t);
                        n_app_created += 1;
                        let t = m.mk_app(g, &[t1,t2]);
                        tmp.push(t);
                        n_app_created += 1;
                    }

                    if long_apps {
                        let t = m.mk_app(f, &[t1; 5]);
                        tmp.push(t);
                        n_app_created += 1;
                        let t = m.mk_app(g, &[t1; 5]);
                        tmp.push(t);
                        n_app_created += 1;
                    }
                }
                i = terms.len();
                terms.extend(&tmp);
                tmp.clear();
            }
        }
        let duration = Instant::now() - start;
        let dur_as_f = duration.as_secs() as f64 + (duration.subsec_micros() as f64 * 1e-6);
        eprintln!("took {:?} to create {} applications \
                  (including long ones: {}, {} in manager, {}/s)",
            duration, n_app_created, long_apps, m.n_apps(), n_app_created as f64 / dur_as_f);
    }

    #[test]
    fn test_stress_apps() {
        mk_stress_app(100, true);
        mk_stress_app(100, false);
        mk_stress_app(1000, false);
        mk_stress_app(1000, false);
    }
}
