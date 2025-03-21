// Test for #129911, which causes a deadlock bug
//
//@ parallel-front-end-robustness
//@ compile-flags: -Z threads=2

fn main() {
    type KooArc = Frc<
        {
            {
                {
                    {};
                }
                type Frc = Frc<{}>::Arc;;
            }
            type Frc = Frc<
                {
                    {
                        {
                            {};
                        }
                        type Frc = Frc<{}>::Arc;;
                    }
                    type Frc = Frc<
                        {
                            {
                                {
                                    {};
                                }
                                type Frc = Frc<{}>::Arc;;
                            }
                            type Frc = Frc<
                                {
                                    {
                                        {
                                            {};
                                        }
                                        type Frc = Frc<{}>::Arc;;
                                    }
                                    type Frc = Frc<
                                        {
                                            {
                                                {
                                                    {
                                                        {};
                                                    }
                                                    type Frc = Frc<{}>::Arc;;
                                                };
                                            }
                                            type Frc = Frc<
                                                {
                                                    {
                                                        {
                                                            {};
                                                        };
                                                    }
                                                    type Frc = Frc<{}>::Arc;;
                                                },
                                            >::Arc;;
                                        },
                                    >::Arc;;
                                },
                            >::Arc;;
                        },
                    >::Arc;;
                },
            >::Arc;;
        },
    >::Arc;
}
