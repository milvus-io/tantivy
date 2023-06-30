/// The tokenizer module contains all of the tools used to process
/// text in `tantivy`.
use tokenizer_api::{BoxTokenStream, TokenFilter, Tokenizer};

use crate::tokenizer::empty_tokenizer::EmptyTokenizer;

/// `TextAnalyzer` tokenizes an input text into tokens and modifies the resulting `TokenStream`.
pub struct TextAnalyzer {
    tokenizer: Box<dyn BoxableTokenizer>,
}


mod public_but_unreachable {
    /// Wrapper to avoid recursive acalls of `box_token_stream`.
    #[derive(Clone)]
    pub struct BoxedTokenizer(pub(super) Box<dyn super::BoxableTokenizer>);
}

use public_but_unreachable::BoxedTokenizer;

impl Tokenizer for BoxedTokenizer {
    type TokenStream<'a> = BoxTokenStream<'a>;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        self.0.box_token_stream(text)
    }
}

impl Clone for Box<dyn BoxableTokenizer> {
    fn clone(&self) -> Self {
        self.box_clone()
    }
}

/// A boxable `Tokenizer`, with its `TokenStream` type erased.
trait BoxableTokenizer: 'static + Send + Sync {
    /// Creates a boxed token stream for a given `str`.
    fn box_token_stream<'a>(&'a mut self, text: &'a str) -> BoxTokenStream<'a>;
    /// Clone this tokenizer.
    fn box_clone(&self) -> Box<dyn BoxableTokenizer>;
}

impl<T: Tokenizer> BoxableTokenizer for T {
    fn box_token_stream<'a>(&'a mut self, text: &'a str) -> BoxTokenStream<'a> {
        BoxTokenStream::new(self.token_stream(text))
    }
    fn box_clone(&self) -> Box<dyn BoxableTokenizer> {
        Box::new(self.clone())
    }
}

impl Clone for TextAnalyzer {
    fn clone(&self) -> Self {
        TextAnalyzer {
            tokenizer: self.tokenizer.box_clone(),
        }
    }
}

impl Default for TextAnalyzer {
    fn default() -> TextAnalyzer {
        TextAnalyzer::from(EmptyTokenizer)
    }
}

impl<T: Tokenizer + Clone> From<T> for TextAnalyzer {
    fn from(tokenizer: T) -> Self {
        TextAnalyzer::builder(tokenizer).build()
    }
}

impl TextAnalyzer {
    /// Create a new TextAnalyzerBuilder
    pub fn builder<T: Tokenizer>(tokenizer: T) -> TextAnalyzerBuilder<T> {
        TextAnalyzerBuilder { tokenizer }
    }

    /// Creates a token stream for a given `str`.
    pub fn token_stream<'a>(&'a mut self, text: &'a str) -> BoxTokenStream<'a> {
        self.tokenizer.box_token_stream(text)
    }
}

/// Builder helper for [`TextAnalyzer`]
pub struct TextAnalyzerBuilder<T=BoxedTokenizer> {
    tokenizer: T,
}

impl<T: Tokenizer> TextAnalyzerBuilder<T> {
    /// Appends a token filter to the current builder.
    ///
    /// # Example
    ///
    /// ```rust
    /// use tantivy::tokenizer::*;
    ///
    /// let en_stem = TextAnalyzer::builder(SimpleTokenizer::default())
    ///     .filter(RemoveLongFilter::limit(40))
    ///     .filter(LowerCaser)
    ///     .filter(Stemmer::default())
    ///     .build();
    /// ```
    pub fn filter<F: TokenFilter>(self, token_filter: F) -> TextAnalyzerBuilder<F::Tokenizer<T>> {
        TextAnalyzerBuilder {
            tokenizer: token_filter.transform(self.tokenizer),
        }
    }

    // Boxes the internal tokenizer. This is useful to write generic code.
    pub fn dynamic(self) -> TextAnalyzerBuilder {
        let boxed_tokenizer = BoxedTokenizer(Box::new(self.tokenizer));
        TextAnalyzerBuilder {
            tokenizer:  boxed_tokenizer,
        }
    }

    /// Apply a filter and returns a boxed version of the TextAnalyzerBuilder.
    /// (If we prefer we can remove this method)
    pub fn filter_dynamic<F: TokenFilter>(self, token_filter: F) -> TextAnalyzerBuilder {
        self.filter(token_filter).dynamic()
    }

    /// Finalize building the TextAnalyzer
    pub fn build(self) -> TextAnalyzer {
        TextAnalyzer {
            tokenizer: Box::new(self.tokenizer),
        }
    }
}


#[cfg(test)]
mod tests {

    use super::*;
    use crate::tokenizer::{AlphaNumOnlyFilter, LowerCaser, RemoveLongFilter, WhitespaceTokenizer};

    #[test]
    fn test_text_analyzer_builder() {
        let mut analyzer = TextAnalyzer::builder(WhitespaceTokenizer::default())
            .filter(AlphaNumOnlyFilter)
            .filter(RemoveLongFilter::limit(6))
            .filter(LowerCaser)
            .build();
        let mut stream = analyzer.token_stream("- first bullet point");
        assert_eq!(stream.next().unwrap().text, "first");
        assert_eq!(stream.next().unwrap().text, "point");
    }



    #[test]
    fn test_text_analyzer_with_filters_boxed() {
        // This test shows how one can build a TextAnalyzer dynamically, by stacking a list
        // of parametrizable token filters.
        //
        // The following enum is the thing that would be serializable.
        // Note that token filters can have their own parameters, too, like the RemoveLongFilter
        enum SerializableTokenFilterEnum {
            LowerCaser(LowerCaser),
            RemoveLongFilter(RemoveLongFilter),
        }
        // Note that everything below is dynamic.
        let filters: Vec<SerializableTokenFilterEnum> = vec![
            SerializableTokenFilterEnum::LowerCaser(LowerCaser),
            SerializableTokenFilterEnum::RemoveLongFilter(RemoveLongFilter::limit(12)),
        ];
        let mut analyzer_builder: TextAnalyzerBuilder = TextAnalyzer::builder(WhitespaceTokenizer::default())
            .dynamic();
        for filter in filters {
            analyzer_builder =
                match filter {
                    SerializableTokenFilterEnum::LowerCaser(lower_caser) =>
                        analyzer_builder.filter_dynamic(lower_caser),
                    SerializableTokenFilterEnum::RemoveLongFilter(remove_long_filter) => {
                        analyzer_builder.filter_dynamic(remove_long_filter)
                },
            }
        }
        let mut analyzer = analyzer_builder.build();
        let mut stream = analyzer.token_stream("first bullet point");
        assert_eq!(stream.next().unwrap().text, "first");
        assert_eq!(stream.next().unwrap().text, "bullet");
    }
}
